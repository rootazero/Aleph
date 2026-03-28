//! Cooldown algorithm — backoff calculations and profile state mutations.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::failure::AuthProfileFailureReason;
use super::store::AuthProfileStore;
use super::normalize_provider_id;

/// Configuration for cooldown behavior
#[derive(Debug, Clone)]
pub struct CooldownConfig {
    /// Base billing backoff duration (default: 5 hours)
    pub billing_backoff: Duration,
    /// Maximum billing backoff duration (default: 24 hours)
    pub billing_max: Duration,
    /// Failure window after which counters reset (default: 24 hours)
    pub failure_window: Duration,
}

impl Default for CooldownConfig {
    fn default() -> Self {
        Self {
            billing_backoff: Duration::from_secs(5 * 60 * 60), // 5 hours
            billing_max: Duration::from_secs(24 * 60 * 60),    // 24 hours
            failure_window: Duration::from_secs(24 * 60 * 60), // 24 hours
        }
    }
}

/// Calculate cooldown duration for rate limit errors.
///
/// Uses base-5 exponential backoff:
/// - 1st error: 1 minute
/// - 2nd error: 5 minutes
/// - 3rd error: 25 minutes
/// - 4th+ error: 1 hour (max)
pub fn calculate_cooldown_ms(error_count: u32) -> u64 {
    let normalized = error_count.max(1);
    let exponent = (normalized - 1).min(3);
    let base_ms = 60 * 1000u64; // 1 minute
    let max_ms = 60 * 60 * 1000u64; // 1 hour

    (base_ms * 5u64.pow(exponent)).min(max_ms)
}

/// Calculate cooldown duration for billing errors.
///
/// Uses base-2 exponential backoff:
/// - 1st error: billing_backoff (default 5 hours)
/// - 2nd error: billing_backoff * 2 (10 hours)
/// - 3rd error: billing_backoff * 4 (20 hours)
/// - Max: billing_max (default 24 hours)
pub fn calculate_billing_cooldown_ms(error_count: u32, config: &CooldownConfig) -> u64 {
    let normalized = error_count.max(1);
    let exponent = (normalized - 1).min(10);
    let base_ms = u64::try_from(config.billing_backoff.as_millis()).unwrap_or(u64::MAX);
    let max_ms = u64::try_from(config.billing_max.as_millis()).unwrap_or(u64::MAX);

    base_ms.saturating_mul(2u64.pow(exponent)).min(max_ms)
}

/// Mark a profile as successfully used
pub fn mark_profile_used(store: &mut AuthProfileStore, profile_id: &str) {
    if !store.profiles.contains_key(profile_id) {
        return;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let stats = store.get_or_create_usage_stats(profile_id);
    stats.last_used = Some(now);
    stats.error_count = Some(0);
    stats.cooldown_until = None;
    stats.disabled_until = None;
    stats.disabled_reason = None;
    stats.failure_counts = None;
}

/// Mark a profile as failed with a specific reason
pub fn mark_profile_failure(
    store: &mut AuthProfileStore,
    profile_id: &str,
    reason: AuthProfileFailureReason,
    config: &CooldownConfig,
) {
    if !store.profiles.contains_key(profile_id) {
        return;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let window_ms = u64::try_from(config.failure_window.as_millis()).unwrap_or(u64::MAX);
    let stats = store.get_or_create_usage_stats(profile_id);

    // Check if failure window expired (reset counters)
    let window_expired = stats
        .last_failure_at
        .is_some_and(|last| last > 0 && now.saturating_sub(last) > window_ms);

    let base_error_count = if window_expired {
        0
    } else {
        stats.error_count.unwrap_or(0)
    };

    let next_error_count = base_error_count + 1;

    // Update failure counts
    let mut failure_counts = if window_expired {
        HashMap::new()
    } else {
        stats.failure_counts.clone().unwrap_or_default()
    };
    *failure_counts.entry(reason).or_insert(0) += 1;

    // Update stats
    stats.error_count = Some(next_error_count);
    stats.failure_counts = Some(failure_counts.clone());
    stats.last_failure_at = Some(now);

    // Apply cooldown based on reason
    if reason == AuthProfileFailureReason::Billing {
        let billing_count = failure_counts.get(&reason).copied().unwrap_or(1);
        let backoff_ms = calculate_billing_cooldown_ms(billing_count, config);
        stats.disabled_until = Some(now + backoff_ms);
        stats.disabled_reason = Some(AuthProfileFailureReason::Billing);
    } else {
        let backoff_ms = calculate_cooldown_ms(next_error_count);
        stats.cooldown_until = Some(now + backoff_ms);
    }
}

/// Clear cooldown for a profile
pub fn clear_profile_cooldown(store: &mut AuthProfileStore, profile_id: &str) {
    if let Some(stats) = store
        .usage_stats
        .as_mut()
        .and_then(|s| s.get_mut(profile_id))
    {
        stats.error_count = Some(0);
        stats.cooldown_until = None;
        stats.disabled_until = None;
        stats.disabled_reason = None;
    }
}

/// Mark a profile as "last good" for a provider
pub fn mark_profile_good(store: &mut AuthProfileStore, profile_id: &str) {
    if let Some(cred) = store.profiles.get(profile_id) {
        let provider = normalize_provider_id(cred.provider());
        store
            .last_good
            .get_or_insert_with(HashMap::new)
            .insert(provider, profile_id.to_string());
    }
}
