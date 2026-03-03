//! mDNS Service Broadcaster
//!
//! Broadcasts the Gateway instance as `_aleph._tcp.local.` service on the local network,
//! enabling automatic discovery by clients without manual configuration.
//!
//! ## Usage
//!
//! ```no_run
//! use alephcore::gateway::MdnsBroadcaster;
//!
//! # async fn example() -> Result<(), String> {
//! let broadcaster = MdnsBroadcaster::new(18790, "aleph")?;
//! // Service is now discoverable on the local network
//! broadcaster.shutdown();
//! # Ok(())
//! # }
//! ```

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::{info, error, warn};

/// mDNS service broadcaster for Gateway discovery
///
/// Registers the Gateway as a `_aleph._tcp.local.` service on the local network,
/// allowing clients to discover instances using mDNS/Zeroconf without manual configuration.
pub struct MdnsBroadcaster {
    daemon: ServiceDaemon,
    service_name: String,
}

impl MdnsBroadcaster {
    /// Create and register a new mDNS service
    ///
    /// # Arguments
    ///
    /// * `port` - The Gateway WebSocket port
    /// * `instance_name` - Unique instance name (e.g., "aleph", "aleph-home")
    ///
    /// # Returns
    ///
    /// `Ok(MdnsBroadcaster)` if registration succeeds, `Err(String)` otherwise.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use alephcore::gateway::MdnsBroadcaster;
    ///
    /// # async fn example() -> Result<(), String> {
    /// let broadcaster = MdnsBroadcaster::new(18790, "aleph")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(port: u16, instance_name: &str) -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;

        let service_type = "_aleph._tcp.local.";
        let hostname = format!("{}.local.", instance_name);

        let service_info = ServiceInfo::new(
            service_type,
            instance_name,
            &hostname,
            (), // No IP address - let mDNS-SD discover local IPs
            port,
            None, // No TXT records for now
        ).map_err(|e| format!("Failed to create service info: {}", e))?;

        daemon.register(service_info)
            .map_err(|e| format!("Failed to register service: {}", e))?;

        info!(
            "mDNS service registered: {} on port {} ({})",
            instance_name, port, service_type
        );

        Ok(Self {
            daemon,
            service_name: instance_name.to_string(),
        })
    }

    /// Shutdown the mDNS broadcaster and unregister the service
    ///
    /// This should be called when the Gateway is shutting down to cleanly
    /// remove the service from the network.
    pub fn shutdown(&self) {
        if let Err(e) = self.daemon.shutdown() {
            error!("Failed to shutdown mDNS daemon: {}", e);
        } else {
            info!("mDNS service '{}' unregistered", self.service_name);
        }
    }
}

impl Drop for MdnsBroadcaster {
    fn drop(&mut self) {
        // Ensure shutdown is called if not explicitly done
        if let Err(e) = self.daemon.shutdown() {
            warn!("mDNS broadcaster dropped without clean shutdown: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcaster_creation() {
        // Note: This test may fail in CI environments without mDNS support
        match MdnsBroadcaster::new(18790, "aleph-test") {
            Ok(broadcaster) => {
                broadcaster.shutdown();
            }
            Err(e) => {
                println!("mDNS not supported in this environment: {}", e);
            }
        }
    }

    #[test]
    fn test_broadcaster_with_custom_instance() {
        match MdnsBroadcaster::new(18790, "aleph-custom-test") {
            Ok(broadcaster) => {
                assert_eq!(broadcaster.service_name, "aleph-custom-test");
                broadcaster.shutdown();
            }
            Err(e) => {
                println!("mDNS not supported in this environment: {}", e);
            }
        }
    }
}
