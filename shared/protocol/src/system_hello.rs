//! system.hello protocol handshake DTOs
//!
//! Pure type definitions shared between `aleph-server` and the NSH desktop client.
//! No I/O or runtime dependencies.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Protocol version for the system.hello handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u32,
    pub minor: u32,
}

/// Client identification block sent by the NSH desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub ui_runtime: String,
}

/// Server identification block returned by aleph-server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub name: String,
    pub version: String,
}

/// Connection limits returned by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloLimits {
    pub max_message_bytes: u64,
    pub max_inflight_requests: u32,
}

/// Optional capabilities negotiated during the system.hello handshake.
///
/// Unknown variant strings from future server versions are preserved on
/// deserialization so they do not break older clients that only understand a
/// subset of features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feature {
    /// Event replay from a stored stream offset.
    EventReplay,
    /// Point-in-time capability snapshots.
    Snapshot,
    /// Binary framing over the WebSocket channel.
    BinaryChannel,
    /// Live server push of admitted event notifications (id-less frames) on a
    /// v1 endpoint. No replay: a gap is recovered by refetching a snapshot.
    LiveEvents,
    /// An unrecognized feature string from a newer version.
    Unknown(String),
}

impl Serialize for Feature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Feature::EventReplay => s.serialize_str("event_replay"),
            Feature::Snapshot => s.serialize_str("snapshot"),
            Feature::BinaryChannel => s.serialize_str("binary_channel"),
            Feature::LiveEvents => s.serialize_str("live_events"),
            Feature::Unknown(name) => s.serialize_str(name),
        }
    }
}

impl<'de> Deserialize<'de> for Feature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "event_replay" => Feature::EventReplay,
            "snapshot" => Feature::Snapshot,
            "binary_channel" => Feature::BinaryChannel,
            "live_events" => Feature::LiveEvents,
            _ => Feature::Unknown(s),
        })
    }
}

impl Feature {
    /// Returns true if this is a known (non-unknown) feature variant.
    pub fn is_known(&self) -> bool {
        matches!(
            self,
            Feature::EventReplay | Feature::Snapshot | Feature::BinaryChannel | Feature::LiveEvents
        )
    }
}

/// Parameters sent by the client in a system.hello request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: ProtocolVersion,
    pub client: ClientHello,
    pub features: Vec<Feature>,
}

/// Result returned by the server for a successful system.hello exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: ProtocolVersion,
    pub server: ServerHello,
    pub connection_id: String,
    pub features: Vec<Feature>,
    pub limits: HelloLimits,
    /// Opaque, stable per-user value for client-side storage keys (Panel
    /// layout). It is not an identity: clients must not derive authority
    /// from it. Absent on legacy `/ws` and when the Gateway has no key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// Select the latest version both peers support within the same major.
pub fn negotiate_version(
    client: &ProtocolVersion,
    server: &ProtocolVersion,
) -> Option<ProtocolVersion> {
    (client.major == server.major).then(|| ProtocolVersion {
        major: server.major,
        minor: client.minor.min(server.minor),
    })
}

/// Select distinct implemented features in the client's preference order.
pub fn negotiate_features(requested: &[Feature], implemented: &[Feature]) -> Vec<Feature> {
    let mut negotiated = Vec::new();
    for feature in requested {
        if feature.is_known() && implemented.contains(feature) && !negotiated.contains(feature) {
            negotiated.push(feature.clone());
        }
    }
    negotiated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_uses_lower_minor_and_rejects_major_mismatch() {
        let v1_0 = ProtocolVersion { major: 1, minor: 0 };
        let v1_3 = ProtocolVersion { major: 1, minor: 3 };
        let v1_max = ProtocolVersion {
            major: 1,
            minor: u32::MAX,
        };
        let v2_0 = ProtocolVersion { major: 2, minor: 0 };
        assert_eq!(negotiate_version(&v1_0, &v1_3), Some(v1_0.clone()));
        assert_eq!(negotiate_version(&v1_max, &v1_0), Some(v1_0.clone()));
        assert_eq!(negotiate_version(&v2_0, &v1_0), None);
    }

    #[test]
    fn features_are_an_ordered_unique_intersection() {
        let requested = [
            Feature::Unknown("future".to_string()),
            Feature::BinaryChannel,
            Feature::Snapshot,
            Feature::BinaryChannel,
            Feature::EventReplay,
        ];
        let implemented = [
            Feature::Snapshot,
            Feature::BinaryChannel,
            Feature::Unknown("future".to_string()),
        ];
        assert_eq!(
            negotiate_features(&requested, &implemented),
            [Feature::BinaryChannel, Feature::Snapshot]
        );
        assert!(negotiate_features(&requested, &[]).is_empty());
    }

    #[test]
    fn hello_params_round_trip() {
        let params = HelloParams {
            protocol: ProtocolVersion { major: 1, minor: 0 },
            client: ClientHello {
                name: "aleph-nsh-desktop".to_string(),
                version: "0.1.0".to_string(),
                platform: "macos".to_string(),
                ui_runtime: "nsh".to_string(),
            },
            features: vec![
                Feature::EventReplay,
                Feature::Snapshot,
                Feature::Unknown("future".to_string()),
            ],
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(
            json["features"],
            serde_json::json!(["event_replay", "snapshot", "future"])
        );
        let back: HelloParams = serde_json::from_value(json).unwrap();
        assert_eq!(back.protocol.major, 1);
        assert_eq!(back.client.name, "aleph-nsh-desktop");
        assert_eq!(back.features, params.features);
    }

    #[test]
    fn hello_result_round_trip() {
        let result = HelloResult {
            protocol: ProtocolVersion { major: 1, minor: 0 },
            server: ServerHello {
                name: "aleph-server".to_string(),
                version: "26.9.1".to_string(),
            },
            connection_id: "conn-001".to_string(),
            features: vec![Feature::EventReplay],
            limits: HelloLimits {
                max_message_bytes: 8_388_608,
                max_inflight_requests: 128,
            },
            subject: Some("0123456789abcdef0123456789abcdef".to_string()),
        };
        let json = serde_json::to_value(&result).unwrap();
        let back: HelloResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.server.name, "aleph-server");
        assert_eq!(back.connection_id, "conn-001");
        assert_eq!(back.subject, result.subject);
    }

    #[test]
    fn hello_result_without_subject_round_trips_for_old_clients() {
        let wire = serde_json::json!({
            "protocol": {"major": 1, "minor": 0},
            "server": {"name": "aleph-server", "version": "26.9.1"},
            "connection_id": "conn-001",
            "features": [],
            "limits": {"max_message_bytes": 1, "max_inflight_requests": 1},
        });
        let back: HelloResult = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(back.subject, None);
        assert_eq!(serde_json::to_value(&back).unwrap(), wire);
    }
}
