use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_HANDSHAKE: &str = "bridge.handshake";
pub const METHOD_PING: &str = "bridge.ping";

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for [`METHOD_HANDSHAKE`].
///
/// The handshake is the first message a freshly spawned helper answers, so it
/// pays for process start-up: dyld, the Swift runtime, and on a cold binary
/// Gatekeeper's signature check. Longer than a ping for that reason alone — the
/// work itself is a version comparison.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Deadline for [`METHOD_PING`]. A live helper answers immediately; anything
/// slower is the answer.
pub const TIMEOUT_MS_PING: u64 = 2_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] = &[(METHOD_PING, TIMEOUT_MS_PING)];

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
            rust_version: "26.4.24".into(),
            protocol_version: 2,
        };
        let j = serde_json::to_string(&req).unwrap();
        let back: HandshakeParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.protocol_version, 2);
    }
}
