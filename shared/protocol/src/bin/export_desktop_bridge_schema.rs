//! Dumps every public desktop-bridge schema to stdout as a single JSON object
//! (`{ "TypeName": <JSONSchema>, ... }`) so the Swift helper's golden-fixtures
//! test (Task 0.10) can validate its handwritten Codable structs against the
//! Rust-side source of truth. See `just bridge-schema`.

#![allow(clippy::print_stdout)]

use std::collections::BTreeMap;

use aleph_protocol::desktop_bridge::{envelope, methods};
use schemars::schema_for;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();

    // Envelope types
    out.insert(
        "Request",
        serde_json::to_value(schema_for!(envelope::Request))?,
    );
    out.insert(
        "Response",
        serde_json::to_value(schema_for!(envelope::Response))?,
    );
    out.insert(
        "ErrorResponse",
        serde_json::to_value(schema_for!(envelope::ErrorResponse))?,
    );
    out.insert(
        "RpcError",
        serde_json::to_value(schema_for!(envelope::RpcError))?,
    );
    out.insert(
        "Notification",
        serde_json::to_value(schema_for!(envelope::Notification))?,
    );

    // bridge.*
    out.insert(
        "HandshakeParams",
        serde_json::to_value(schema_for!(methods::bridge::HandshakeParams))?,
    );
    out.insert(
        "HandshakeResult",
        serde_json::to_value(schema_for!(methods::bridge::HandshakeResult))?,
    );
    out.insert(
        "PingParams",
        serde_json::to_value(schema_for!(methods::bridge::PingParams))?,
    );
    out.insert(
        "PingResult",
        serde_json::to_value(schema_for!(methods::bridge::PingResult))?,
    );

    // screen.*
    out.insert(
        "CaptureParams",
        serde_json::to_value(schema_for!(methods::screen::CaptureParams))?,
    );
    out.insert(
        "CaptureResult",
        serde_json::to_value(schema_for!(methods::screen::CaptureResult))?,
    );
    out.insert(
        "OcrParams",
        serde_json::to_value(schema_for!(methods::screen::OcrParams))?,
    );
    out.insert(
        "OcrResult",
        serde_json::to_value(schema_for!(methods::screen::OcrResult))?,
    );
    out.insert(
        "ListDisplaysResult",
        serde_json::to_value(schema_for!(methods::screen::ListDisplaysResult))?,
    );

    // window.* is deliberately absent: window listing / focusing / app launch is
    // done in-process by the limb (`desktop/shared/src/action/window.rs`), never
    // over the bridge, so the Swift helper has no window Codable structs to
    // validate against.

    // input.*
    //
    // Results are exported too, not just params: each one now carries the
    // `delivery` field ("targeted" | "global"), and the Swift helper's Codable
    // structs have to agree on it or the model would be told which rail ran by
    // a struct that cannot say.
    out.insert(
        "ClickParams",
        serde_json::to_value(schema_for!(methods::input::ClickParams))?,
    );
    out.insert(
        "ClickResult",
        serde_json::to_value(schema_for!(methods::input::ClickResult))?,
    );
    out.insert(
        "TypeTextParams",
        serde_json::to_value(schema_for!(methods::input::TypeTextParams))?,
    );
    out.insert(
        "TypeTextResult",
        serde_json::to_value(schema_for!(methods::input::TypeTextResult))?,
    );
    out.insert(
        "KeyComboParams",
        serde_json::to_value(schema_for!(methods::input::KeyComboParams))?,
    );
    out.insert(
        "KeyComboResult",
        serde_json::to_value(schema_for!(methods::input::KeyComboResult))?,
    );
    out.insert(
        "KeyButtonParams",
        serde_json::to_value(schema_for!(methods::input::KeyButtonParams))?,
    );
    out.insert(
        "KeyButtonResult",
        serde_json::to_value(schema_for!(methods::input::KeyButtonResult))?,
    );
    out.insert(
        "ScrollParams",
        serde_json::to_value(schema_for!(methods::input::ScrollParams))?,
    );
    out.insert(
        "ScrollResult",
        serde_json::to_value(schema_for!(methods::input::ScrollResult))?,
    );
    out.insert(
        "DragParams",
        serde_json::to_value(schema_for!(methods::input::DragParams))?,
    );
    out.insert(
        "DragResult",
        serde_json::to_value(schema_for!(methods::input::DragResult))?,
    );
    out.insert(
        "HoverParams",
        serde_json::to_value(schema_for!(methods::input::HoverParams))?,
    );
    out.insert(
        "HoverResult",
        serde_json::to_value(schema_for!(methods::input::HoverResult))?,
    );
    out.insert(
        "MouseButtonParams",
        serde_json::to_value(schema_for!(methods::input::MouseButtonParams))?,
    );
    out.insert(
        "MouseButtonResult",
        serde_json::to_value(schema_for!(methods::input::MouseButtonResult))?,
    );
    out.insert(
        "CursorPositionResult",
        serde_json::to_value(schema_for!(methods::input::CursorPositionResult))?,
    );

    // media.camera.*
    out.insert(
        "SnapParams",
        serde_json::to_value(schema_for!(methods::media::SnapParams))?,
    );
    out.insert(
        "SnapResult",
        serde_json::to_value(schema_for!(methods::media::SnapResult))?,
    );
    out.insert(
        "ClipParams",
        serde_json::to_value(schema_for!(methods::media::ClipParams))?,
    );
    out.insert(
        "ClipResult",
        serde_json::to_value(schema_for!(methods::media::ClipResult))?,
    );

    // media.audio.*
    out.insert(
        "ListAudioDevicesParams",
        serde_json::to_value(schema_for!(methods::media::ListAudioDevicesParams))?,
    );
    out.insert(
        "ListAudioDevicesResult",
        serde_json::to_value(schema_for!(methods::media::ListAudioDevicesResult))?,
    );
    out.insert(
        "AudioDeviceInfo",
        serde_json::to_value(schema_for!(methods::media::AudioDeviceInfo))?,
    );
    out.insert(
        "RecordAudioParams",
        serde_json::to_value(schema_for!(methods::media::RecordAudioParams))?,
    );
    out.insert(
        "RecordAudioResult",
        serde_json::to_value(schema_for!(methods::media::RecordAudioResult))?,
    );

    // media.speech.*
    out.insert(
        "TranscribeFileParams",
        serde_json::to_value(schema_for!(methods::media::TranscribeFileParams))?,
    );
    out.insert(
        "TranscribeFileResult",
        serde_json::to_value(schema_for!(methods::media::TranscribeFileResult))?,
    );

    let json = serde_json::to_string_pretty(&out)?;
    println!("{json}");
    Ok(())
}
