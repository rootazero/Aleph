use serde::{Deserialize, Serialize};

/// Trust level for remote agents — determines auth requirements and permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// localhost — no auth required, full permissions
    Local,
    /// LAN / paired — token required, configured permissions
    Trusted,
    /// Internet — OAuth2/mTLS required, restricted permissions
    Public,
}

impl TrustLevel {
    /// Infer trust level from a socket address
    pub fn infer_from_addr(addr: &std::net::SocketAddr) -> Self {
        let ip = addr.ip();
        if ip.is_loopback() {
            TrustLevel::Local
        } else if is_private_ip(&ip) {
            TrustLevel::Trusted
        } else {
            TrustLevel::Public
        }
    }

    /// Infer trust level from a URL
    pub fn infer_from_url(url: &str) -> Self {
        if let Ok(parsed) = url::Url::parse(url) {
            // Use host() to handle IPv6 addresses correctly (host_str includes brackets)
            match parsed.host() {
                Some(url::Host::Domain("localhost")) => TrustLevel::Local,
                Some(url::Host::Ipv4(ip)) if ip.is_loopback() => TrustLevel::Local,
                Some(url::Host::Ipv6(ip)) if ip.is_loopback() => TrustLevel::Local,
                Some(url::Host::Ipv4(ip)) if ip.is_private() || ip.is_link_local() => {
                    TrustLevel::Trusted
                }
                Some(url::Host::Domain(host)) if is_private_hostname(host) => TrustLevel::Trusted,
                _ => TrustLevel::Public,
            }
        } else {
            TrustLevel::Public
        }
    }
}

/// Security scheme for A2A authentication (A2A spec compliant)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SecurityScheme {
    ApiKey {
        location: ApiKeyLocation,
        name: String,
    },
    Http {
        scheme: String,
        #[serde(
            rename = "bearerFormat",
            skip_serializing_if = "Option::is_none"
        )]
        bearer_format: Option<String>,
    },
    OAuth2 {
        flows: serde_json::Value,
    },
    OpenIdConnect {
        #[serde(rename = "connectUrl")]
        connect_url: String,
    },
}

/// Location where an API key is sent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

/// Credentials extracted from an incoming request
#[derive(Debug, Clone)]
pub enum Credentials {
    BearerToken(String),
    ApiKey(String),
    OAuth2Token(String),
    None,
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            let octets = v6.octets();
            (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80) // link-local fe80::/10
                || (octets[0] & 0xfe) == 0xfc // ULA fc00::/7
        }
    }
}

fn is_private_hostname(host: &str) -> bool {
    host.ends_with(".local")
        || host.ends_with(".lan")
        || host.starts_with("192.168.")
        || host.starts_with("10.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn trust_level_serde_roundtrip() {
        for level in [TrustLevel::Local, TrustLevel::Trusted, TrustLevel::Public] {
            let json = serde_json::to_string(&level).unwrap();
            let back: TrustLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, back);
        }
    }

    #[test]
    fn trust_level_json_values() {
        assert_eq!(serde_json::to_string(&TrustLevel::Local).unwrap(), "\"local\"");
        assert_eq!(serde_json::to_string(&TrustLevel::Trusted).unwrap(), "\"trusted\"");
        assert_eq!(serde_json::to_string(&TrustLevel::Public).unwrap(), "\"public\"");
    }

    #[test]
    fn infer_from_addr_loopback_v4() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Local);
    }

    #[test]
    fn infer_from_addr_loopback_v6() {
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080);
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Local);
    }

    #[test]
    fn infer_from_addr_private_ip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 8080);
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Trusted);

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080);
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Trusted);
    }

    #[test]
    fn infer_from_addr_link_local_v6() {
        let addr = SocketAddr::new(
            IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap()),
            8080,
        );
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Trusted);
    }

    #[test]
    fn infer_from_addr_public_ip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 8080);
        assert_eq!(TrustLevel::infer_from_addr(&addr), TrustLevel::Public);
    }

    #[test]
    fn infer_from_url_localhost() {
        assert_eq!(TrustLevel::infer_from_url("http://localhost:8080"), TrustLevel::Local);
        assert_eq!(TrustLevel::infer_from_url("http://127.0.0.1:3000"), TrustLevel::Local);
        assert_eq!(TrustLevel::infer_from_url("http://[::1]:3000"), TrustLevel::Local);
    }

    #[test]
    fn infer_from_url_lan() {
        assert_eq!(TrustLevel::infer_from_url("http://myhost.local:8080"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::infer_from_url("http://server.lan:8080"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::infer_from_url("http://192.168.1.50:8080"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::infer_from_url("http://10.0.0.1:8080"), TrustLevel::Trusted);
    }

    #[test]
    fn infer_from_url_public() {
        assert_eq!(TrustLevel::infer_from_url("https://api.example.com"), TrustLevel::Public);
        assert_eq!(TrustLevel::infer_from_url("https://8.8.8.8:443"), TrustLevel::Public);
    }

    #[test]
    fn infer_from_url_invalid() {
        assert_eq!(TrustLevel::infer_from_url("not a url"), TrustLevel::Public);
    }

    #[test]
    fn security_scheme_api_key_serde() {
        let scheme = SecurityScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-API-Key".to_string(),
        };
        let json = serde_json::to_value(&scheme).unwrap();
        assert_eq!(json["type"], "apiKey");
        assert_eq!(json["location"], "header");
        assert_eq!(json["name"], "X-API-Key");

        let back: SecurityScheme = serde_json::from_value(json).unwrap();
        assert!(matches!(back, SecurityScheme::ApiKey { .. }));
    }

    #[test]
    fn security_scheme_http_serde() {
        let scheme = SecurityScheme::Http {
            scheme: "bearer".to_string(),
            bearer_format: Some("JWT".to_string()),
        };
        let json = serde_json::to_value(&scheme).unwrap();
        assert_eq!(json["type"], "http");
        assert_eq!(json["scheme"], "bearer");
        assert_eq!(json["bearerFormat"], "JWT");
    }
}
