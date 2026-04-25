use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_HANDSHAKE: &str = "bridge.handshake";
pub const METHOD_PING: &str = "bridge.ping";
pub const METHOD_SHUTDOWN: &str = "bridge.shutdown";
pub const SUGGESTED_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HandshakeParams {
    pub rust_version: String,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HandshakeResult {
    pub swift_version: String,
    pub protocol_version: u32,
    pub supported_methods: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PingParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PingResult {
    pub pong: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_roundtrip() {
        let req = HandshakeParams {
            rust_version: "2026.04.24".into(),
            protocol_version: 2,
        };
        let j = serde_json::to_string(&req).unwrap();
        let back: HandshakeParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.protocol_version, 2);
    }
}
