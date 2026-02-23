//! iMessage Target Parsing
//!
//! Handles parsing and normalization of iMessage targets (phone numbers, emails, chat IDs).
//!
//! # Supported Formats
//!
//! - Phone number: `+15551234567`, `555-123-4567`
//! - Email: `user@example.com`
//! - Chat ID: `chat_id:123`
//! - Service-prefixed: `imessage:+15551234567`, `sms:+15551234567`

use std::fmt;

/// Service type for message delivery
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum Service {
    /// Auto-detect (prefer iMessage, fall back to SMS)
    #[default]
    Auto,
    /// Force iMessage
    IMessage,
    /// Force SMS
    Sms,
}


impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Service::Auto => write!(f, "auto"),
            Service::IMessage => write!(f, "iMessage"),
            Service::Sms => write!(f, "SMS"),
        }
    }
}

/// Parsed iMessage target
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IMessageTarget {
    /// Phone number target
    Phone {
        /// Normalized phone number (E.164 format)
        number: String,
        /// Service to use
        service: Service,
    },
    /// Email target
    Email {
        /// Email address (lowercase)
        email: String,
    },
    /// Chat ID target (for group chats)
    ChatId {
        /// Numeric chat ID from database
        id: i64,
    },
    /// Chat GUID target
    ChatGuid {
        /// Chat GUID string
        guid: String,
    },
}

impl IMessageTarget {
    /// Get the target string for sending
    pub fn to_target_string(&self) -> String {
        match self {
            IMessageTarget::Phone { number, .. } => number.clone(),
            IMessageTarget::Email { email } => email.clone(),
            IMessageTarget::ChatId { id } => format!("chat_id:{}", id),
            IMessageTarget::ChatGuid { guid } => guid.clone(),
        }
    }
}

/// Parse error
#[derive(Debug, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
pub enum ParseError {
    #[error("Invalid target format: {0}")]
    InvalidFormat(String),

    #[error("Invalid phone number: {0}")]
    InvalidPhone(String),

    #[error("Invalid chat ID: {0}")]
    InvalidChatId(String),
}

/// Parse an iMessage target string
///
/// # Supported Formats
///
/// - `+15551234567` - Phone number (E.164)
/// - `5551234567` - Phone number (will be normalized)
/// - `user@example.com` - Email address
/// - `chat_id:123` - Group chat by numeric ID
/// - `chat_guid:ABC-123` - Group chat by GUID
/// - `imessage:+15551234567` - Force iMessage service
/// - `sms:+15551234567` - Force SMS service
///
/// # Examples
///
/// ```ignore
/// let target = parse_target("+15551234567")?;
/// assert!(matches!(target, IMessageTarget::Phone { .. }));
///
/// let target = parse_target("chat_id:42")?;
/// assert!(matches!(target, IMessageTarget::ChatId { id: 42 }));
/// ```
pub fn parse_target(target: &str) -> Result<IMessageTarget, ParseError> {
    let target = target.trim();

    if target.is_empty() {
        return Err(ParseError::InvalidFormat("Empty target".to_string()));
    }

    // Check for prefixed formats
    if let Some(rest) = target.strip_prefix("chat_id:") {
        let id: i64 = rest
            .parse()
            .map_err(|_| ParseError::InvalidChatId(rest.to_string()))?;
        return Ok(IMessageTarget::ChatId { id });
    }

    if let Some(rest) = target.strip_prefix("chat_guid:") {
        return Ok(IMessageTarget::ChatGuid {
            guid: rest.to_string(),
        });
    }

    if let Some(rest) = target.strip_prefix("imessage:") {
        let normalized = normalize_phone(rest);
        return Ok(IMessageTarget::Phone {
            number: normalized,
            service: Service::IMessage,
        });
    }

    if let Some(rest) = target.strip_prefix("sms:") {
        let normalized = normalize_phone(rest);
        return Ok(IMessageTarget::Phone {
            number: normalized,
            service: Service::Sms,
        });
    }

    // Check if it looks like an email
    if target.contains('@') && target.contains('.') {
        return Ok(IMessageTarget::Email {
            email: target.to_lowercase(),
        });
    }

    // Assume it's a phone number
    let normalized = normalize_phone(target);
    if normalized.is_empty() {
        return Err(ParseError::InvalidPhone(target.to_string()));
    }

    Ok(IMessageTarget::Phone {
        number: normalized,
        service: Service::Auto,
    })
}

/// Normalize a phone number to E.164 format
///
/// - Removes non-digit characters (except leading +)
/// - Adds country code if missing (assumes US +1)
///
/// # Examples
///
/// ```ignore
/// assert_eq!(normalize_phone("+1 555-123-4567"), "+15551234567");
/// assert_eq!(normalize_phone("5551234567"), "+15551234567");
/// assert_eq!(normalize_phone("(555) 123-4567"), "+15551234567");
/// ```
pub fn normalize_phone(phone: &str) -> String {
    let phone = phone.trim();

    // Extract digits only, preserving leading +
    let has_plus = phone.starts_with('+');
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();

    if digits.is_empty() {
        return String::new();
    }

    // Add country code if needed
    if has_plus {
        format!("+{}", digits)
    } else if digits.len() == 10 {
        // Assume US number, add +1
        format!("+1{}", digits)
    } else if digits.len() == 11 && digits.starts_with('1') {
        // US number with 1 prefix
        format!("+{}", digits)
    } else {
        // Unknown format, just add +
        format!("+{}", digits)
    }
}

/// Check if a sender is in the allowlist
pub fn is_allowed_sender(sender: &str, allowlist: &[String]) -> bool {
    let normalized = normalize_phone(sender);

    allowlist.iter().any(|allowed| {
        let allowed_normalized = normalize_phone(allowed);
        // Check both original and normalized forms
        sender == allowed
            || sender.to_lowercase() == allowed.to_lowercase()
            || (!normalized.is_empty()
                && !allowed_normalized.is_empty()
                && normalized == allowed_normalized)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_phone_number() {
        let target = parse_target("+15551234567").unwrap();
        assert!(matches!(
            target,
            IMessageTarget::Phone {
                number,
                service: Service::Auto
            } if number == "+15551234567"
        ));
    }

    #[test]
    fn test_parse_phone_without_plus() {
        let target = parse_target("5551234567").unwrap();
        if let IMessageTarget::Phone { number, .. } = target {
            assert_eq!(number, "+15551234567");
        } else {
            panic!("Expected Phone target");
        }
    }

    #[test]
    fn test_parse_email() {
        let target = parse_target("user@example.com").unwrap();
        assert!(matches!(
            target,
            IMessageTarget::Email { email } if email == "user@example.com"
        ));
    }

    #[test]
    fn test_parse_chat_id() {
        let target = parse_target("chat_id:42").unwrap();
        assert!(matches!(target, IMessageTarget::ChatId { id: 42 }));
    }

    #[test]
    fn test_parse_service_prefix() {
        let target = parse_target("imessage:+15551234567").unwrap();
        assert!(matches!(
            target,
            IMessageTarget::Phone {
                service: Service::IMessage,
                ..
            }
        ));

        let target = parse_target("sms:+15551234567").unwrap();
        assert!(matches!(
            target,
            IMessageTarget::Phone {
                service: Service::Sms,
                ..
            }
        ));
    }

    #[test]
    fn test_normalize_phone() {
        assert_eq!(normalize_phone("+1 555-123-4567"), "+15551234567");
        assert_eq!(normalize_phone("5551234567"), "+15551234567");
        assert_eq!(normalize_phone("(555) 123-4567"), "+15551234567");
        assert_eq!(normalize_phone("1-555-123-4567"), "+15551234567");
        assert_eq!(normalize_phone("+44 20 7946 0958"), "+442079460958");
    }

    #[test]
    fn test_is_allowed_sender() {
        let allowlist = vec![
            "+15551234567".to_string(),
            "user@example.com".to_string(),
        ];

        assert!(is_allowed_sender("+15551234567", &allowlist));
        assert!(is_allowed_sender("5551234567", &allowlist)); // Normalized match
        assert!(is_allowed_sender("user@example.com", &allowlist));
        assert!(is_allowed_sender("USER@EXAMPLE.COM", &allowlist)); // Case insensitive
        assert!(!is_allowed_sender("+19998887777", &allowlist));
    }
}
