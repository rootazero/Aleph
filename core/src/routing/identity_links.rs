//! Cross-channel user identity linking.
//!
//! Maps user IDs across channels to a canonical identity,
//! allowing sessions to be shared across platforms.

use std::collections::HashMap;

/// Resolve a peer ID to its canonical identity via identity links.
///
/// Checks both bare peer ID and channel-scoped peer ID.
/// Returns the canonical name if a link is found, None otherwise.
pub fn resolve_linked_peer_id(
    identity_links: &HashMap<String, Vec<String>>,
    channel: &str,
    peer_id: &str,
) -> Option<String> {
    let peer_lower = peer_id.trim().to_lowercase();
    if peer_lower.is_empty() {
        return None;
    }

    let channel_lower = channel.trim().to_lowercase();
    let scoped = if channel_lower.is_empty() {
        None
    } else {
        Some(format!("{}:{}", channel_lower, peer_lower))
    };

    // Sort by canonical name for deterministic resolution when multiple matches exist
    let mut sorted_links: Vec<_> = identity_links.iter().collect();
    sorted_links.sort_by_key(|(k, _)| (*k).clone());

    for (canonical, ids) in sorted_links {
        let canonical_name = canonical.trim();
        if canonical_name.is_empty() {
            continue;
        }

        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower.is_empty() {
                continue;
            }

            if id_lower == peer_lower {
                return Some(canonical_name.to_string());
            }

            if let Some(ref scoped_id) = scoped {
                if &id_lower == scoped_id {
                    return Some(canonical_name.to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_links() -> HashMap<String, Vec<String>> {
        let mut links = HashMap::new();
        links.insert(
            "john".to_string(),
            vec![
                "telegram:123456".to_string(),
                "discord:789012".to_string(),
                "slack:U345678".to_string(),
            ],
        );
        links.insert(
            "alice".to_string(),
            vec![
                "telegram:654321".to_string(),
                "imessage:+1234567890".to_string(),
            ],
        );
        links
    }

    #[test]
    fn test_resolve_scoped_match() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "telegram", "123456"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_cross_channel() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "discord", "789012"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_no_match() {
        let links = test_links();
        assert_eq!(resolve_linked_peer_id(&links, "slack", "unknown"), None);
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "TELEGRAM", "123456"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_empty_inputs() {
        let links = test_links();
        assert_eq!(resolve_linked_peer_id(&links, "", "123456"), None);
        assert_eq!(resolve_linked_peer_id(&links, "telegram", ""), None);
    }

    #[test]
    fn test_resolve_empty_links() {
        let links = HashMap::new();
        assert_eq!(resolve_linked_peer_id(&links, "telegram", "123"), None);
    }
}
