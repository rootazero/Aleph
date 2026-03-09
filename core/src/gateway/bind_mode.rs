//! Bind Mode Configuration
//!
//! Controls which network interface the gateway listens on.

use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Determines the network interface address the gateway binds to.
///
/// - `Loopback`: Bind to 127.0.0.1 (localhost only, most secure).
/// - `Lan`: Bind to 0.0.0.0 (all interfaces, accessible from LAN).
/// - `Tailnet`: Bind to the Tailscale IP (future: auto-detect via `tailscale status`).
/// - `Auto`: Automatically choose the best bind address (currently falls back to loopback).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BindMode {
    #[default]
    Loopback,
    Lan,
    Tailnet,
    Auto,
}

impl BindMode {
    /// Resolve the bind address for the given port.
    ///
    /// `Tailnet` and `Auto` currently fall back to loopback (127.0.0.1).
    /// Real Tailscale IP resolution is planned for a future iteration.
    pub fn resolve_addr(&self, port: u16) -> SocketAddr {
        let ip: IpAddr = match self {
            Self::Loopback | Self::Tailnet | Self::Auto => {
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
            }
            Self::Lan => IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        };
        SocketAddr::new(ip, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_mode_default() {
        assert_eq!(BindMode::default(), BindMode::Loopback);
    }

    #[test]
    fn test_bind_mode_resolve_addr() {
        let port = 18790u16;
        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let all = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

        assert_eq!(BindMode::Loopback.resolve_addr(port), loopback);
        assert_eq!(BindMode::Lan.resolve_addr(port), all);
        assert_eq!(BindMode::Tailnet.resolve_addr(port), loopback, "Tailnet should fallback to loopback");
        assert_eq!(BindMode::Auto.resolve_addr(port), loopback, "Auto should fallback to loopback");
    }

    #[test]
    fn test_bind_mode_deserialize() {
        #[derive(Deserialize)]
        struct Wrapper {
            mode: BindMode,
        }

        let w: Wrapper = toml::from_str(r#"mode = "loopback""#).unwrap();
        assert_eq!(w.mode, BindMode::Loopback);

        let w: Wrapper = toml::from_str(r#"mode = "lan""#).unwrap();
        assert_eq!(w.mode, BindMode::Lan);

        let w: Wrapper = toml::from_str(r#"mode = "tailnet""#).unwrap();
        assert_eq!(w.mode, BindMode::Tailnet);

        let w: Wrapper = toml::from_str(r#"mode = "auto""#).unwrap();
        assert_eq!(w.mode, BindMode::Auto);
    }
}
