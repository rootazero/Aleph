//! Profile ordering — resolve which profile to use next for a provider.

use std::time::{SystemTime, UNIX_EPOCH};

use super::store::AuthProfileStore;
use super::normalize_provider_id;

/// Resolve the profile order for a provider.
///
/// Ordering logic:
/// 1. Partition profiles into available vs in-cooldown
/// 2. Sort available by type (OAuth > Token > API Key)
/// 3. Within each type, sort by lastUsed (oldest first = round-robin)
/// 4. Append cooldown profiles sorted by expiry (soonest first)
/// 5. If preferred_profile is specified, put it first
pub fn resolve_profile_order(
    store: &AuthProfileStore,
    provider: &str,
    explicit_order: Option<&[String]>,
    preferred_profile: Option<&str>,
) -> Vec<String> {
    let provider_key = normalize_provider_id(provider);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Get base order
    let base_order: Vec<String> = if let Some(order) = explicit_order {
        order.to_vec()
    } else if let Some(stored_order) = store.order.as_ref().and_then(|o| {
        o.iter()
            .find(|(k, _)| normalize_provider_id(k) == provider_key)
            .map(|(_, v)| v.clone())
    }) {
        stored_order
    } else {
        store.list_profiles_for_provider(provider)
    };

    if base_order.is_empty() {
        return Vec::new();
    }

    // Filter to valid profiles
    let filtered: Vec<String> = base_order
        .into_iter()
        .filter(|profile_id| {
            let Some(cred) = store.profiles.get(profile_id) else {
                return false;
            };
            if normalize_provider_id(cred.provider()) != provider_key {
                return false;
            }
            cred.is_valid() && !cred.is_expired()
        })
        .collect();

    // Deduplicate
    let mut deduped: Vec<String> = Vec::new();
    for id in filtered {
        if !deduped.contains(&id) {
            deduped.push(id);
        }
    }

    // Partition into available and in-cooldown
    let mut available: Vec<String> = Vec::new();
    let mut in_cooldown: Vec<(String, u64)> = Vec::new();

    for profile_id in deduped {
        let cooldown_until = store
            .get_usage_stats(&profile_id)
            .and_then(|s| s.unusable_until())
            .unwrap_or(0);

        if cooldown_until > 0 && now < cooldown_until {
            in_cooldown.push((profile_id, cooldown_until));
        } else {
            available.push(profile_id);
        }
    }

    // Sort available by type score, then by lastUsed (oldest first)
    let mut scored: Vec<(String, u8, u64)> = available
        .into_iter()
        .map(|profile_id| {
            let type_score = store
                .profiles
                .get(&profile_id)
                .map(|c| c.type_score())
                .unwrap_or(3);
            let last_used = store
                .get_usage_stats(&profile_id)
                .and_then(|s| s.last_used)
                .unwrap_or(0);
            (profile_id, type_score, last_used)
        })
        .collect();

    scored.sort_by(|a, b| {
        // Primary: type score (lower = higher priority)
        a.1.cmp(&b.1)
            // Secondary: lastUsed (oldest first for round-robin)
            .then_with(|| a.2.cmp(&b.2))
    });

    let sorted: Vec<String> = scored.into_iter().map(|(id, _, _)| id).collect();

    // Sort cooldown profiles by expiry (soonest first)
    in_cooldown.sort_by_key(|(_, until)| *until);
    let cooldown_sorted: Vec<String> = in_cooldown.into_iter().map(|(id, _)| id).collect();

    // Combine: available first, then cooldown
    let mut result: Vec<String> = sorted;
    result.extend(cooldown_sorted);

    // Put preferred profile first if specified
    if let Some(preferred) = preferred_profile {
        if result.contains(&preferred.to_string()) {
            result.retain(|id| id != preferred);
            result.insert(0, preferred.to_string());
        }
    }

    result
}
