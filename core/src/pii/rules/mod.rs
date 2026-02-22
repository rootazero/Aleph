//! PII detection rules
//!
//! Each rule detects a specific type of PII with precision-tuned patterns.
//! Rules are ordered by severity (Critical first) to ensure higher-severity
//! matches win in overlap deduplication.

mod api_key;
mod bank_card;
mod email;
mod id_card;
mod ip_address;
mod phone;
mod ssh_key;

use crate::pii::engine::{PiiMatch, PiiSeverity};

/// Trait for PII detection rules
pub trait PiiRule: Send + Sync {
    /// Rule identifier (matches config field name)
    fn name(&self) -> &str;

    /// Severity level of this PII type
    fn severity(&self) -> PiiSeverity;

    /// Placeholder text for replacement
    fn placeholder(&self) -> &str;

    /// Detect PII in text, returning all matches
    fn detect(&self, text: &str) -> Vec<PiiMatch>;
}

/// Build all rules ordered by severity (Critical first)
pub fn build_rules() -> Vec<Box<dyn PiiRule>> {
    vec![
        Box::new(api_key::ApiKeyRule::new()),
        Box::new(ssh_key::SshKeyRule::new()),
        Box::new(id_card::IdCardRule::new()),
        Box::new(phone::PhoneRule::new()),
        Box::new(bank_card::BankCardRule::new()),
        Box::new(email::EmailRule::new()),
        Box::new(ip_address::IpAddressRule::new()),
    ]
}
