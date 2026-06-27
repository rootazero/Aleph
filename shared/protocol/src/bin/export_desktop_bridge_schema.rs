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

    // window.*
    out.insert(
        "WindowListParams",
        serde_json::to_value(schema_for!(methods::window::ListParams))?,
    );
    out.insert(
        "WindowListResult",
        serde_json::to_value(schema_for!(methods::window::ListResult))?,
    );
    out.insert(
        "WindowFocusParams",
        serde_json::to_value(schema_for!(methods::window::FocusParams))?,
    );

    // input.*
    out.insert(
        "ClickParams",
        serde_json::to_value(schema_for!(methods::input::ClickParams))?,
    );
    out.insert(
        "TypeTextParams",
        serde_json::to_value(schema_for!(methods::input::TypeTextParams))?,
    );
    out.insert(
        "KeyComboParams",
        serde_json::to_value(schema_for!(methods::input::KeyComboParams))?,
    );
    out.insert(
        "ScrollParams",
        serde_json::to_value(schema_for!(methods::input::ScrollParams))?,
    );
    out.insert(
        "DragParams",
        serde_json::to_value(schema_for!(methods::input::DragParams))?,
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
