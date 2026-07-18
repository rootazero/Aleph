//! SHA-256 leaf fingerprint + display-only cert parsing (SAN / subject).

use crate::cert_trust::CertInfo;
use sha2::{Digest, Sha256};

/// Colon-grouped uppercase hex SHA-256 of the leaf DER — matches
/// `openssl x509 -fingerprint -sha256`.
#[must_use]
pub fn fingerprint_sha256(leaf_der: &[u8]) -> String {
    let digest = Sha256::digest(leaf_der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Render a SAN `GeneralName` for display. `IPAddress` gets special-cased to
/// dotted-decimal / IPv6 text (x509-parser's own `Display` renders it as
/// colon-hex bytes, which isn't what a human — or a fingerprint prompt —
/// wants to compare against a URL's host). Never panics: falls back to the
/// crate's `Display` impl for anything that doesn't parse as 4 or 16 bytes.
fn general_name_to_string(gn: &x509_parser::extensions::GeneralName<'_>) -> String {
    use x509_parser::extensions::GeneralName;
    match gn {
        GeneralName::DNSName(s) | GeneralName::RFC822Name(s) | GeneralName::URI(s) => {
            (*s).to_string()
        }
        GeneralName::IPAddress(bytes) => <[u8; 4]>::try_from(*bytes)
            .map(|octets| std::net::Ipv4Addr::from(octets).to_string())
            .or_else(|_| {
                <[u8; 16]>::try_from(*bytes)
                    .map(|octets| std::net::Ipv6Addr::from(octets).to_string())
            })
            .unwrap_or_else(|_| gn.to_string()),
        other => other.to_string(),
    }
}

/// Parse SAN + subject for display. Never fails hard — on a parse error the
/// returned info has empty SAN/subject but still carries the reason, so the
/// prompt can still show the fingerprint.
#[must_use]
pub fn parse_cert_info(leaf_der: &[u8], reason: &str) -> CertInfo {
    use x509_parser::prelude::*;
    let (subject, sans) = match X509Certificate::from_der(leaf_der) {
        Ok((_, cert)) => {
            let subject = cert.subject().to_string();
            let sans = cert
                .subject_alternative_name()
                .ok()
                .flatten()
                .map(|ext| {
                    ext.value
                        .general_names
                        .iter()
                        .map(general_name_to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (subject, sans)
        }
        Err(_) => (String::new(), Vec::new()),
    };
    CertInfo {
        sans,
        subject,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a self-signed cert DER at test time (rcgen is already a workspace dep
    // used by gateway/tls.rs). SAN includes an IP so parse_cert_info sees it.
    fn sample_der() -> Vec<u8> {
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(vec!["172.245.43.211".to_string()]).unwrap();
        cert.der().to_vec()
    }

    #[test]
    fn fingerprint_is_colon_grouped_uppercase_hex_32_bytes() {
        let fp = fingerprint_sha256(&sample_der());
        let groups: Vec<&str> = fp.split(':').collect();
        assert_eq!(groups.len(), 32, "sha256 = 32 bytes");
        assert!(groups.iter().all(|g| g.len() == 2
            && g.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())));
    }

    #[test]
    fn parse_cert_info_extracts_ip_san() {
        let info = parse_cert_info(&sample_der(), "self-signed");
        assert!(info.sans.iter().any(|s| s.contains("172.245.43.211")));
        assert_eq!(info.reason, "self-signed");
    }

    #[test]
    fn parse_cert_info_on_malformed_der_is_empty_not_panic() {
        let info = parse_cert_info(&[0xDE, 0xAD, 0xBE, 0xEF], "self-signed");
        assert!(info.sans.is_empty());
        assert!(info.subject.is_empty());
        assert_eq!(info.reason, "self-signed");
    }
}
