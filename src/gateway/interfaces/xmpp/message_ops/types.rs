//! XMPP type definitions and JID parsing.

/// Parsed JID (Jabber ID) components.
///
/// A full JID has the form: `local@domain/resource`
/// - `local@domain` is the bare JID
/// - `/resource` is optional
#[derive(Debug, Clone, PartialEq)]
pub struct JidParts {
    /// User/local part (before @)
    pub local: String,
    /// Server domain (between @ and /)
    pub domain: String,
    /// Optional resource (after /)
    pub resource: Option<String>,
}

impl JidParts {
    /// Reconstruct the bare JID (without resource)
    pub fn bare(&self) -> String {
        format!("{}@{}", self.local, self.domain)
    }
}

/// Parsed XMPP message stanza.
#[derive(Debug, Clone, PartialEq)]
pub struct XmppMessage {
    /// Sender JID
    pub from: String,
    /// Message body text
    pub body: String,
    /// Message type: "chat" or "groupchat"
    pub msg_type: String,
    /// Optional thread ID
    pub thread: Option<String>,
    /// Optional message ID
    pub id: Option<String>,
}

/// Parse a JID string into its components.
///
/// Supports formats:
/// - `local@domain`
/// - `local@domain/resource`
///
/// Returns `None` if the JID doesn't contain `@`.
pub fn parse_jid(jid: &str) -> Option<JidParts> {
    let at_pos = jid.find('@')?;
    let local = jid[..at_pos].to_string();
    let remainder = &jid[at_pos + 1..];

    let (domain, resource) = if let Some(slash_pos) = remainder.find('/') {
        (
            remainder[..slash_pos].to_string(),
            Some(remainder[slash_pos + 1..].to_string()),
        )
    } else {
        (remainder.to_string(), None)
    };

    if local.is_empty() || domain.is_empty() {
        return None;
    }

    Some(JidParts {
        local,
        domain,
        resource,
    })
}
