//! Privacy filter middleware for sensitive data redaction.
//!
//! Automatically filters sensitive information before state events are
//! published to the event bus. Supports:
//! - Password fields (AXSecureTextField)
//! - Credit card numbers (Luhn algorithm)
//! - SSN patterns
//! - Sensitive applications (1Password, Keychain, etc.)
//!
//! # Example
//!
//! ```ignore
//! let filter = PrivacyFilter::new(config);
//! filter.filter(&mut state_event);  // Redacts sensitive data in-place
//! ```

use super::types::{AppState, Element};
use regex::Regex;
use std::collections::HashSet;
use tracing::warn;

/// Privacy filter configuration.
#[derive(Debug, Clone)]
pub struct PrivacyFilterConfig {
    /// Sensitive application bundle IDs (complete blackout)
    pub sensitive_apps: HashSet<String>,

    /// Sensitive AX roles
    pub sensitive_roles: HashSet<String>,

    /// Enable credit card detection
    pub filter_credit_cards: bool,

    /// Enable SSN detection
    pub filter_ssn: bool,

    /// Enable phone number detection
    pub filter_phone: bool,

    /// Audit log path (optional)
    pub audit_log_path: Option<String>,
}

impl Default for PrivacyFilterConfig {
    fn default() -> Self {
        let mut sensitive_apps = HashSet::new();
        sensitive_apps.insert("com.agilebits.onepassword7".to_string());
        sensitive_apps.insert("com.apple.keychainaccess".to_string());
        sensitive_apps.insert("com.tencent.xinWeChat".to_string());

        let mut sensitive_roles = HashSet::new();
        sensitive_roles.insert("AXSecureTextField".to_string());

        Self {
            sensitive_apps,
            sensitive_roles,
            filter_credit_cards: true,
            filter_ssn: true,
            filter_phone: true,
            audit_log_path: None,
        }
    }
}

/// Privacy filter middleware.
pub struct PrivacyFilter {
    config: PrivacyFilterConfig,
    #[allow(dead_code)] // Will be used when credit card filtering is implemented
    credit_card_regex: Regex,
    ssn_regex: Regex,
    phone_regex: Regex,
}

impl PrivacyFilter {
    /// Create a new privacy filter with configuration.
    pub fn new(config: PrivacyFilterConfig) -> Self {
        Self {
            config,
            credit_card_regex: Regex::new(r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b").unwrap(),
            ssn_regex: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),
            phone_regex: Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b").unwrap(),
        }
    }

    /// Filter sensitive data from application state.
    ///
    /// Modifies the state in-place, redacting sensitive information.
    pub fn filter(&self, state: &mut AppState) -> bool {
        let mut filtered = false;

        // Rule 1: Sensitive applications (complete blackout)
        if self.config.sensitive_apps.contains(&state.app_id) {
            state.elements.clear();
            state.app_context = None;
            filtered = true;

            if let Some(ref path) = self.config.audit_log_path {
                self.log_filtered_app(&state.app_id, path);
            }

            return filtered;
        }

        // Rule 2: Filter elements
        for element in &mut state.elements {
            if self.filter_element(element) {
                filtered = true;
            }
        }

        filtered
    }

    /// Filter a single element.
    fn filter_element(&self, element: &mut Element) -> bool {
        let mut filtered = false;

        // Password fields
        if self.config.sensitive_roles.contains(&element.role) {
            if let Some(ref mut value) = element.current_value {
                *value = "***".to_string();
                filtered = true;
            }
            element.label = None;
        }

        // Pattern matching on current_value
        if let Some(ref mut value) = element.current_value {
            if self.config.filter_credit_cards && self.looks_like_credit_card(value) {
                *value = "****-****-****-****".to_string();
                filtered = true;
            } else if self.config.filter_ssn && self.ssn_regex.is_match(value) {
                *value = "***-**-****".to_string();
                filtered = true;
            } else if self.config.filter_phone && self.phone_regex.is_match(value) {
                *value = "***-***-****".to_string();
                filtered = true;
            }
        }

        filtered
    }

    /// Check if a string looks like a credit card number.
    fn looks_like_credit_card(&self, s: &str) -> bool {
        // Extract digits only
        let digits: String = s.chars().filter(|c| c.is_numeric()).collect();

        // Check length (13-19 digits for valid cards)
        if digits.len() < 13 || digits.len() > 19 {
            return false;
        }

        // Luhn algorithm check
        Self::luhn_check(&digits)
    }

    /// Luhn algorithm for credit card validation.
    fn luhn_check(digits: &str) -> bool {
        let mut sum = 0;
        let mut double = false;

        for ch in digits.chars().rev() {
            if let Some(digit) = ch.to_digit(10) {
                let mut value = digit;
                if double {
                    value *= 2;
                    if value > 9 {
                        value -= 9;
                    }
                }
                sum += value;
                double = !double;
            } else {
                return false;
            }
        }

        sum % 10 == 0
    }

    /// Log filtered application to audit log.
    fn log_filtered_app(&self, app_id: &str, log_path: &str) {
        warn!(
            "Privacy filter: blocked sensitive app {} (audit log: {})",
            app_id, log_path
        );
        // TODO: Write to actual audit log file
    }
}

impl Default for PrivacyFilter {
    fn default() -> Self {
        Self::new(PrivacyFilterConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::state_bus::types::{ElementSource, ElementState, StateSource};

    fn create_test_element(role: &str, value: Option<&str>) -> Element {
        Element {
            id: "test_001".to_string(),
            role: role.to_string(),
            label: Some("Test".to_string()),
            current_value: value.map(|s| s.to_string()),
            rect: None,
            state: ElementState::default(),
            source: ElementSource::Ax,
            confidence: 1.0,
        }
    }

    #[test]
    fn test_filter_password_field() {
        let filter = PrivacyFilter::default();
        let mut element = create_test_element("AXSecureTextField", Some("my-password"));

        filter.filter_element(&mut element);

        assert_eq!(element.current_value.as_ref().unwrap(), "***");
        assert!(element.label.is_none());
    }

    #[test]
    fn test_filter_credit_card() {
        let filter = PrivacyFilter::default();
        let mut element = create_test_element("textfield", Some("4111-1111-1111-1111"));

        filter.filter_element(&mut element);

        assert_eq!(element.current_value.as_ref().unwrap(), "****-****-****-****");
    }

    #[test]
    fn test_filter_ssn() {
        let filter = PrivacyFilter::default();
        let mut element = create_test_element("textfield", Some("123-45-6789"));

        filter.filter_element(&mut element);

        assert_eq!(element.current_value.as_ref().unwrap(), "***-**-****");
    }

    #[test]
    fn test_filter_phone() {
        let filter = PrivacyFilter::default();
        let mut element = create_test_element("textfield", Some("555-123-4567"));

        filter.filter_element(&mut element);

        assert_eq!(element.current_value.as_ref().unwrap(), "***-***-****");
    }

    #[test]
    fn test_luhn_check_valid() {
        // Valid Visa test cards (Stripe)
        assert!(PrivacyFilter::luhn_check("4111111111111111"));
        assert!(PrivacyFilter::luhn_check("4242424242424242"));
    }

    #[test]
    fn test_luhn_check_invalid() {
        assert!(!PrivacyFilter::luhn_check("1234567890123456"));
    }

    #[test]
    fn test_filter_sensitive_app() {
        let filter = PrivacyFilter::default();
        let mut state = AppState {
            app_id: "com.agilebits.onepassword7".to_string(),
            elements: vec![create_test_element("button", Some("test"))],
            app_context: Some(serde_json::json!({"key": "value"})),
            source: StateSource::Accessibility,
            confidence: 1.0,
        };

        let filtered = filter.filter(&mut state);

        assert!(filtered);
        assert!(state.elements.is_empty());
        assert!(state.app_context.is_none());
    }

    #[test]
    fn test_no_filter_normal_text() {
        let filter = PrivacyFilter::default();
        let mut element = create_test_element("textfield", Some("Hello, world!"));

        let filtered = filter.filter_element(&mut element);

        assert!(!filtered);
        assert_eq!(element.current_value.as_ref().unwrap(), "Hello, world!");
    }
}
