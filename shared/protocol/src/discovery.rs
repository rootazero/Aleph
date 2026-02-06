//! Service discovery types

use serde::{Deserialize, Serialize};

/// Discovered Aleph instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredInstance {
    /// Instance name
    pub name: String,
    /// Hostname (e.g., "aleph.local")
    pub hostname: String,
    /// Port
    pub port: u16,
    /// IP addresses
    pub addresses: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_instance_serde() {
        let instance = DiscoveredInstance {
            name: "Home Aleph".to_string(),
            hostname: "aleph.local".to_string(),
            port: 18789,
            addresses: vec!["192.168.1.100".to_string()],
        };
        let json = serde_json::to_string(&instance).unwrap();
        let parsed: DiscoveredInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.port, 18789);
    }
}
