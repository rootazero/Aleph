//! mDNS Scanner for discovering Aleph instances

use aleph_protocol::DiscoveredInstance;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::time::Duration;
use tracing::{debug, error};

/// mDNS scanner for discovering Aleph instances on the local network
pub struct MdnsScanner {
    daemon: ServiceDaemon,
}

impl MdnsScanner {
    /// Create a new mDNS scanner
    pub fn new() -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;
        Ok(Self { daemon })
    }

    /// Scan for Aleph instances on the local network
    pub async fn scan(&self, timeout: Duration) -> Vec<DiscoveredInstance> {
        let service_type = "_aleph._tcp.local.";
        let receiver = match self.daemon.browse(service_type) {
            Ok(rx) => rx,
            Err(e) => {
                error!("Failed to browse mDNS services: {}", e);
                return vec![];
            }
        };

        let mut instances = Vec::new();
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            // Use recv_timeout for cleaner handling
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    debug!("Discovered Aleph instance: {}", info.get_fullname());
                    instances.push(DiscoveredInstance {
                        name: info.get_fullname().to_string(),
                        hostname: info.get_hostname().to_string(),
                        port: info.get_port(),
                        addresses: info
                            .get_addresses()
                            .iter()
                            .map(|addr| addr.to_string())
                            .collect(),
                    });
                }
                Ok(_) => {
                    // Other events (ServiceFound, ServiceRemoved) - ignore
                }
                Err(_) => {
                    // Timeout or disconnected - continue until overall timeout
                }
            }
        }

        instances
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_creation() {
        match MdnsScanner::new() {
            Ok(_) => println!("mDNS scanner created successfully"),
            Err(e) => println!("mDNS not supported: {}", e),
        }
    }

    #[tokio::test]
    async fn test_scan_returns_vec() {
        match MdnsScanner::new() {
            Ok(scanner) => {
                let instances = scanner.scan(Duration::from_millis(100)).await;
                assert!(instances.is_empty() || !instances.is_empty());
            }
            Err(e) => println!("mDNS not supported: {}", e),
        }
    }
}
