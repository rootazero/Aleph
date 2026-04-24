//! Dumps every public desktop-bridge schema to stdout as a single JSON object
//! (`{ "TypeName": <JSONSchema>, ... }`) so the Swift helper's golden-fixtures
//! test (Task 0.10) can validate its handwritten Codable structs against the
//! Rust-side source of truth. See `just bridge-schema`.

use std::collections::BTreeMap;

use aleph_protocol::desktop_bridge::{envelope, methods};
use schemars::schema_for;

fn main() {
    let mut out: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();

    // Envelope types
    out.insert(
        "Request",
        serde_json::to_value(schema_for!(envelope::Request)).unwrap(),
    );
    out.insert(
        "Response",
        serde_json::to_value(schema_for!(envelope::Response)).unwrap(),
    );
    out.insert(
        "ErrorResponse",
        serde_json::to_value(schema_for!(envelope::ErrorResponse)).unwrap(),
    );
    out.insert(
        "RpcError",
        serde_json::to_value(schema_for!(envelope::RpcError)).unwrap(),
    );
    out.insert(
        "Notification",
        serde_json::to_value(schema_for!(envelope::Notification)).unwrap(),
    );

    // bridge.*
    out.insert(
        "HandshakeParams",
        serde_json::to_value(schema_for!(methods::bridge::HandshakeParams)).unwrap(),
    );
    out.insert(
        "HandshakeResult",
        serde_json::to_value(schema_for!(methods::bridge::HandshakeResult)).unwrap(),
    );
    out.insert(
        "PingParams",
        serde_json::to_value(schema_for!(methods::bridge::PingParams)).unwrap(),
    );
    out.insert(
        "PingResult",
        serde_json::to_value(schema_for!(methods::bridge::PingResult)).unwrap(),
    );

    // screen.*
    out.insert(
        "CaptureParams",
        serde_json::to_value(schema_for!(methods::screen::CaptureParams)).unwrap(),
    );
    out.insert(
        "CaptureResult",
        serde_json::to_value(schema_for!(methods::screen::CaptureResult)).unwrap(),
    );
    out.insert(
        "OcrParams",
        serde_json::to_value(schema_for!(methods::screen::OcrParams)).unwrap(),
    );
    out.insert(
        "OcrResult",
        serde_json::to_value(schema_for!(methods::screen::OcrResult)).unwrap(),
    );
    out.insert(
        "ListDisplaysResult",
        serde_json::to_value(schema_for!(methods::screen::ListDisplaysResult)).unwrap(),
    );

    // window.*
    out.insert(
        "WindowListParams",
        serde_json::to_value(schema_for!(methods::window::ListParams)).unwrap(),
    );
    out.insert(
        "WindowListResult",
        serde_json::to_value(schema_for!(methods::window::ListResult)).unwrap(),
    );
    out.insert(
        "WindowFocusParams",
        serde_json::to_value(schema_for!(methods::window::FocusParams)).unwrap(),
    );

    // input.*
    out.insert(
        "ClickParams",
        serde_json::to_value(schema_for!(methods::input::ClickParams)).unwrap(),
    );
    out.insert(
        "TypeTextParams",
        serde_json::to_value(schema_for!(methods::input::TypeTextParams)).unwrap(),
    );
    out.insert(
        "KeyComboParams",
        serde_json::to_value(schema_for!(methods::input::KeyComboParams)).unwrap(),
    );
    out.insert(
        "ScrollParams",
        serde_json::to_value(schema_for!(methods::input::ScrollParams)).unwrap(),
    );
    out.insert(
        "DragParams",
        serde_json::to_value(schema_for!(methods::input::DragParams)).unwrap(),
    );

    // media.camera.*
    out.insert(
        "SnapParams",
        serde_json::to_value(schema_for!(methods::media::SnapParams)).unwrap(),
    );
    out.insert(
        "SnapResult",
        serde_json::to_value(schema_for!(methods::media::SnapResult)).unwrap(),
    );
    out.insert(
        "ClipParams",
        serde_json::to_value(schema_for!(methods::media::ClipParams)).unwrap(),
    );
    out.insert(
        "ClipResult",
        serde_json::to_value(schema_for!(methods::media::ClipResult)).unwrap(),
    );

    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
