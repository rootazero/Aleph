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
    /// An unrecognized feature string from a newer version.
    Unknown(String),
}

impl Serialize for Feature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Feature::EventReplay => s.serialize_str("event_replay"),
            Feature::Snapshot => s.serialize_str("snapshot"),
            Feature::BinaryChannel => s.serialize_str("binary_channel"),
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
            _ => Feature::Unknown(s),
        })
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
            features: vec![Feature::EventReplay, Feature::Snapshot],
        };
        let json = serde_json::to_value(&params).unwrap();
        let back: HelloParams = serde_json::from_value(json).unwrap();
        assert_eq!(back.protocol.major, 1);
        assert_eq!(back.client.name, "aleph-nsh-desktop");
        assert_eq!(back.features.len(), 2);
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
            limits: HelloLimits { max_message_bytes: 8_388_608, max_inflight_requests: 128 },
        };
        let json = serde_json::to_value(&result).unwrap();
        let back: HelloResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.server.name, "aleph-server");
        assert_eq!(back.connection_id, "conn-001");
    }
}
