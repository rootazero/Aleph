//! Property-based tests for channel types serde roundtrip.
//!
//! Uses proptest to verify serialization/deserialization correctness
//! for channel identifiers, status, pairing data, and capabilities.

use proptest::prelude::*;
use proptest::collection::vec as prop_vec;

use super::channel::{ChannelCapabilities, ChannelId, ChannelStatus, PairingData};

// ============================================================================
// Strategies
// ============================================================================

/// Strategy for ChannelId (non-empty strings).
fn arb_channel_id() -> impl Strategy<Value = ChannelId> {
    "[a-zA-Z0-9:_.-]{1,40}".prop_map(ChannelId::new)
}

/// Strategy for ChannelStatus.
fn arb_channel_status() -> impl Strategy<Value = ChannelStatus> {
    prop_oneof![
        Just(ChannelStatus::Disconnected),
        Just(ChannelStatus::Connecting),
        Just(ChannelStatus::Connected),
        Just(ChannelStatus::Error),
        Just(ChannelStatus::Disabled),
    ]
}

/// Strategy for PairingData.
fn arb_pairing_data() -> impl Strategy<Value = PairingData> {
    prop_oneof![
        Just(PairingData::None),
        "[a-zA-Z0-9]{4,12}".prop_map(PairingData::Code),
        "[a-zA-Z0-9+/=]{10,60}".prop_map(PairingData::QrCode),
    ]
}

/// Strategy for ChannelCapabilities.
/// Uses a fixed-size boolean vec to avoid proptest's tuple size limits.
fn arb_channel_capabilities() -> impl Strategy<Value = ChannelCapabilities> {
    (
        prop_vec(any::<bool>(), 11..=11),  // exactly 11 booleans
        0..100_000usize,                   // max_message_length
        0..100_000_000u64,                 // max_attachment_size
    )
        .prop_map(|(bools, max_msg, max_att)| {
            ChannelCapabilities {
                attachments: bools[0],
                images: bools[1],
                audio: bools[2],
                video: bools[3],
                reactions: bools[4],
                replies: bools[5],
                editing: bools[6],
                deletion: bools[7],
                typing_indicator: bools[8],
                read_receipts: bools[9],
                rich_text: bools[10],
                max_message_length: max_msg,
                max_attachment_size: max_att,
            }
        })
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// ChannelId::Display output matches the inner string.
    #[test]
    fn channel_id_display_matches_inner(s in "[a-zA-Z0-9:_.-]{1,40}") {
        let id = ChannelId::new(&s);

        // Display should produce the same string as the inner value
        let displayed = format!("{}", id);
        prop_assert_eq!(&displayed, &s);

        // as_str should also match
        prop_assert_eq!(id.as_str(), s.as_str());
    }

    /// ChannelStatus: serde roundtrip preserves value.
    #[test]
    fn channel_status_serde_roundtrip(status in arb_channel_status()) {
        let json_str = serde_json::to_string(&status).unwrap();
        let parsed: ChannelStatus = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(parsed, status);
    }

    /// PairingData: serde roundtrip preserves variant and content.
    #[test]
    fn pairing_data_serde_roundtrip(data in arb_pairing_data()) {
        let json_str = serde_json::to_string(&data).unwrap();
        let parsed: PairingData = serde_json::from_str(&json_str).unwrap();

        // Compare variant and inner data
        match (&parsed, &data) {
            (PairingData::None, PairingData::None) => {}
            (PairingData::Code(a), PairingData::Code(b)) => {
                prop_assert_eq!(a, b);
            }
            (PairingData::QrCode(a), PairingData::QrCode(b)) => {
                prop_assert_eq!(a, b);
            }
            _ => prop_assert!(false, "PairingData variant mismatch after roundtrip"),
        }
    }

    /// PairingData tagged serialization uses expected JSON structure.
    #[test]
    fn pairing_data_tagged_format(data in arb_pairing_data()) {
        let json_str = serde_json::to_string(&data).unwrap();
        let json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // The serde tag format uses "type" field
        let type_field = json_val.get("type").and_then(|v| v.as_str());

        match &data {
            PairingData::None => {
                prop_assert_eq!(type_field, Some("none"));
            }
            PairingData::Code(_) => {
                prop_assert_eq!(type_field, Some("code"));
                prop_assert!(json_val.get("data").is_some());
            }
            PairingData::QrCode(_) => {
                prop_assert_eq!(type_field, Some("qr_code"));
                prop_assert!(json_val.get("data").is_some());
            }
        }
    }

    /// ChannelCapabilities defaults are minimal (all booleans false, sizes zero).
    #[test]
    fn channel_capabilities_defaults_are_minimal(_dummy in 0..1u8) {
        let caps = ChannelCapabilities::default();

        prop_assert!(!caps.attachments);
        prop_assert!(!caps.images);
        prop_assert!(!caps.audio);
        prop_assert!(!caps.video);
        prop_assert!(!caps.reactions);
        prop_assert!(!caps.replies);
        prop_assert!(!caps.editing);
        prop_assert!(!caps.deletion);
        prop_assert!(!caps.typing_indicator);
        prop_assert!(!caps.read_receipts);
        prop_assert!(!caps.rich_text);
        prop_assert_eq!(caps.max_message_length, 0);
        prop_assert_eq!(caps.max_attachment_size, 0);
    }

    /// ChannelCapabilities: serde roundtrip preserves all fields.
    #[test]
    fn channel_capabilities_serde_roundtrip(caps in arb_channel_capabilities()) {
        let json_str = serde_json::to_string(&caps).unwrap();
        let parsed: ChannelCapabilities = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(parsed.attachments, caps.attachments);
        prop_assert_eq!(parsed.images, caps.images);
        prop_assert_eq!(parsed.audio, caps.audio);
        prop_assert_eq!(parsed.video, caps.video);
        prop_assert_eq!(parsed.reactions, caps.reactions);
        prop_assert_eq!(parsed.replies, caps.replies);
        prop_assert_eq!(parsed.editing, caps.editing);
        prop_assert_eq!(parsed.deletion, caps.deletion);
        prop_assert_eq!(parsed.typing_indicator, caps.typing_indicator);
        prop_assert_eq!(parsed.read_receipts, caps.read_receipts);
        prop_assert_eq!(parsed.rich_text, caps.rich_text);
        prop_assert_eq!(parsed.max_message_length, caps.max_message_length);
        prop_assert_eq!(parsed.max_attachment_size, caps.max_attachment_size);
    }

    /// ChannelId: serde roundtrip preserves inner value.
    #[test]
    fn channel_id_serde_roundtrip(id in arb_channel_id()) {
        let json_str = serde_json::to_string(&id).unwrap();
        let parsed: ChannelId = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(parsed.as_str(), id.as_str());
    }
}
