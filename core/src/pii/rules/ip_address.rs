//! IPv4 address detection

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static IP_RE: OnceLock<Regex> = OnceLock::new();

fn ip_regex() -> &'static Regex {
    IP_RE.get_or_init(|| {
        Regex::new(
            r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b",
        )
        .unwrap()
    })
}

pub struct IpAddressRule;

impl IpAddressRule {
    pub fn new() -> Self {
        Self
    }
}

impl PiiRule for IpAddressRule {
    fn name(&self) -> &str {
        "ip_address"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Low
    }
    fn placeholder(&self) -> &str {
        "[IP]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = ip_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: m.start(),
                end: m.end(),
                matched_text: m.as_str().to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> IpAddressRule {
        IpAddressRule::new()
    }

    #[test]
    fn test_detect_ipv4() {
        let matches = rule().detect("Server at 192.168.1.100");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "192.168.1.100");
    }

    #[test]
    fn test_detect_public_ip() {
        let matches = rule().detect("Remote: 203.0.113.42");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_invalid_octet() {
        let matches = rule().detect("Not an IP: 999.168.1.1");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_partial() {
        let matches = rule().detect("version 1.2.3");
        assert_eq!(matches.len(), 0);
    }
}
