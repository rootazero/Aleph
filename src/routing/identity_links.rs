//! Cross-channel user identity linking.
//!
//! Maps user IDs across channels to a canonical identity,
//! allowing sessions to be shared across platforms.

use std::collections::HashMap;

/// Validate identity links for duplicate IDs across canonical names.
/// Logs a warning for each ID that appears in more than one canonical.
pub fn validate_identity_links(identity_links: &HashMap<String, Vec<String>>) {
    let mut id_to_canonicals: std::collections::HashMap<String, Vec<&str>> = HashMap::new();

    for (canonical, ids) in identity_links.iter() {
        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower.is_empty() {
                continue;
            }
            id_to_canonicals
                .entry(id_lower.clone())
                .or_default()
                .push(canonical.as_str());
        }
    }

    for (id, canonicals) in id_to_canonicals {
        if canonicals.len() > 1 {
            tracing::warn!(
                id,
                canonicals = ?canonicals,
                "identity link ID appears under multiple canonical names; first match wins at runtime"
            );
        }
    }
}

/// Resolve a peer ID to its canonical identity via identity links.
///
/// Checks both bare peer ID and channel-scoped peer ID.
/// Returns the canonical name if a link is found, None otherwise.
pub(crate) fn resolve_linked_peer_id(
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
    sorted_links.sort_by(|(a, _), (b, _)| a.cmp(b));

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

    #[test]
    fn test_duplicate_id_across_canonicals_picks_first() {
        // When two canonicals share the same ID, the first one in sorted order wins.
        // Sorting by canonical name makes resolution deterministic.
        let mut links = HashMap::new();
        links.insert("alice".to_string(), vec!["telegram:123".to_string()]);
        links.insert("bob".to_string(), vec!["telegram:123".to_string()]); // duplicate ID

        let result = resolve_linked_peer_id(&links, "telegram", "123");
        // alice < bob alphabetically, so alice is returned
        assert_eq!(result, Some("alice".to_string()));
    }
}
