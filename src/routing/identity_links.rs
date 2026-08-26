//! Cross-channel user identity linking.
//!
//! Maps user IDs across channels to a canonical identity,
//! allowing sessions to be shared across platforms.

use std::collections::HashMap;
use std::fmt;

/// A configured ID maps to more than one canonical user — this would let the
/// alphabetic tie-break in `resolve_linked_peer_id` flip ownership across
/// config reloads, which is a cross-user data leak.
#[derive(Debug)]
pub struct DuplicateIdentityLinkError {
    pub id: String,
    pub canonicals: Vec<String>,
}

impl fmt::Display for DuplicateIdentityLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "identity link ID '{}' appears under multiple canonical names {:?}; \
             each cross-channel ID must belong to exactly one canonical to prevent \
             tie-break drift across config reloads",
            self.id, self.canonicals
        )
    }
}

impl std::error::Error for DuplicateIdentityLinkError {}

/// Validate identity links — every (channel, id) pair must resolve to exactly
/// one canonical. Returns the first conflict so callers (deserialize hooks)
/// can reject the whole config rather than load a broken state.
pub fn validate_identity_links(
    identity_links: &HashMap<String, Vec<String>>,
) -> Result<(), DuplicateIdentityLinkError> {
    let mut id_to_canonicals: HashMap<String, Vec<String>> = HashMap::new();

    for (canonical, ids) in identity_links.iter() {
        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower.is_empty() {
                continue;
            }
            id_to_canonicals
                .entry(id_lower)
                .or_default()
                .push(canonical.clone());
        }
    }

    // Sort the (id, canonicals) pairs for deterministic error selection so
    // tests don't depend on HashMap iteration order.
    let mut pairs: Vec<(String, Vec<String>)> = id_to_canonicals.into_iter().collect();
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (id, mut canonicals) in pairs {
        if canonicals.len() > 1 {
            canonicals.sort();
            return Err(DuplicateIdentityLinkError { id, canonicals });
        }
    }

    Ok(())
}

/// Resolve a peer ID to its canonical identity via identity links.
///
/// Checks both bare peer ID and channel-scoped peer ID.
/// Returns the canonical name if a link is found, None otherwise.
///
/// Assumes `identity_links` has been validated by `validate_identity_links`
/// at deserialize time (no ID maps to more than one canonical). Directly
/// constructed maps are handled fail-closed below: an ID that maps to more than
/// one non-empty canonical is ignored instead of being assigned by iteration
/// order.
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
        Some(format!("{channel_lower}:{peer_lower}"))
    };

    let mut id_to_canonical: HashMap<String, Option<String>> = HashMap::new();
    for (canonical, ids) in identity_links {
        let canonical_name = canonical.trim();
        if canonical_name.is_empty() {
            continue;
        }
        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower.is_empty() {
                continue;
            }
            match id_to_canonical.get(&id_lower).cloned() {
                Some(existing) if existing.as_deref() == Some(canonical_name) => {}
                Some(_) => {
                    id_to_canonical.insert(id_lower, None);
                }
                None => {
                    id_to_canonical.insert(id_lower, Some(canonical_name.to_string()));
                }
            }
        }
    }

    let canonical_for = |id: &str| {
        let id_lower = id.trim().to_lowercase();
        id_to_canonical
            .get(&id_lower)
            .and_then(Option::as_ref)
            .map(String::as_str)
    };

    // Prefer a scoped match (more specific) over a bare peer-ID match.
    if let Some(scoped_id) = scoped.as_deref() {
        if let Some(canonical_name) = canonical_for(scoped_id) {
            return Some(canonical_name.to_string());
        }
    }

    canonical_for(&peer_lower).map(|canonical| canonical.to_string())
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
    fn test_duplicate_id_across_canonicals_is_ignored() {
        // Configs loaded via serde reject this before resolution, but the
        // resolver also fails closed for directly-constructed maps so a
        // mutation cannot choose one user's canonical for another user's ID.
        let mut links = HashMap::new();
        links.insert("alice".to_string(), vec!["telegram:123".to_string()]);
        links.insert("bob".to_string(), vec!["telegram:123".to_string()]);

        let result = resolve_linked_peer_id(&links, "telegram", "123");
        assert_eq!(result, None);
    }

    #[test]
    fn validate_accepts_unique_ids() {
        let links = test_links();
        assert!(validate_identity_links(&links).is_ok());
    }

    #[test]
    fn validate_accepts_empty_map() {
        let links = HashMap::new();
        assert!(validate_identity_links(&links).is_ok());
    }

    #[test]
    fn validate_rejects_duplicate_id_across_canonicals() {
        let mut links = HashMap::new();
        links.insert("alice".to_string(), vec!["telegram:123".to_string()]);
        links.insert("bob".to_string(), vec!["telegram:123".to_string()]);

        let err = validate_identity_links(&links).unwrap_err();
        assert_eq!(err.id, "telegram:123");
        assert_eq!(err.canonicals, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn validate_rejects_duplicate_id_case_insensitive() {
        // Case-insensitive comparison must catch upper/lower variants.
        let mut links = HashMap::new();
        links.insert("alice".to_string(), vec!["Telegram:ABC".to_string()]);
        links.insert("bob".to_string(), vec!["telegram:abc".to_string()]);

        let err = validate_identity_links(&links).unwrap_err();
        assert_eq!(err.id, "telegram:abc");
    }

    #[test]
    fn validate_ignores_empty_strings() {
        let mut links = HashMap::new();
        links.insert("alice".to_string(), vec!["".to_string(), "  ".to_string()]);
        links.insert("bob".to_string(), vec!["".to_string()]);
        // Empty IDs are skipped — no duplicate is reported.
        assert!(validate_identity_links(&links).is_ok());
    }
}
