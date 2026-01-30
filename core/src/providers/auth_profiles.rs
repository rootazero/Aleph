//! Auth profile management for API key rotation.
//!
//! Provides:
//! - Multiple credential types (API key, token, OAuth)
//! - Per-profile usage tracking with cooldown support
//! - Round-robin profile ordering with type preference
//! - Exponential backoff for rate limits and billing errors
//!
//! Reference: Moltbot src/agents/auth-profiles/

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Current store version for migrations
pub const AUTH_STORE_VERSION: u32 = 1;

// ============================================================================
// Credential Types
// ============================================================================

/// API key credential (static key)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiKeyCredential {
    pub provider: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Token credential (bearer-style, optionally expiring)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenCredential {
    pub provider: String,
    pub token: String,
    /// Optional expiry timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// OAuth credential (refreshable)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthCredential {
    pub provider: String,
    pub access: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Expiry timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Auth profile credential (discriminated union)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthProfileCredential {
    ApiKey(ApiKeyCredential),
    Token(TokenCredential),
    OAuth(OAuthCredential),
}

impl AuthProfileCredential {
    /// Get the provider ID for this credential
    pub fn provider(&self) -> &str {
        match self {
            Self::ApiKey(c) => &c.provider,
            Self::Token(c) => &c.provider,
            Self::OAuth(c) => &c.provider,
        }
    }

    /// Get the credential type name
    pub fn credential_type(&self) -> &'static str {
        match self {
            Self::ApiKey(_) => "api_key",
            Self::Token(_) => "token",
            Self::OAuth(_) => "oauth",
        }
    }

    /// Check if the credential has valid/non-empty authentication data
    pub fn is_valid(&self) -> bool {
        match self {
            Self::ApiKey(c) => !c.key.trim().is_empty(),
            Self::Token(c) => !c.token.trim().is_empty(),
            Self::OAuth(c) => !c.access.trim().is_empty() || c.refresh.as_ref().is_some_and(|r| !r.trim().is_empty()),
        }
    }

    /// Check if the credential is expired (for token/oauth types)
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        match self {
            Self::ApiKey(_) => false, // API keys don't expire
            Self::Token(c) => c.expires.is_some_and(|exp| exp > 0 && now >= exp),
            Self::OAuth(c) => {
                // OAuth is expired only if no refresh token and access is expired
                c.refresh.is_none() && c.expires.is_some_and(|exp| exp > 0 && now >= exp)
            }
        }
    }

    /// Get the API key or token for use in requests
    pub fn resolve_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(c) => Some(&c.key),
            Self::Token(c) => Some(&c.token),
            Self::OAuth(c) => Some(&c.access),
        }
    }

    /// Type score for ordering (lower = higher priority)
    /// OAuth > Token > API Key
    pub fn type_score(&self) -> u8 {
        match self {
            Self::OAuth(_) => 0,
            Self::Token(_) => 1,
            Self::ApiKey(_) => 2,
        }
    }
}

// ============================================================================
// Failure Tracking
// ============================================================================

/// Reason for auth profile failure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProfileFailureReason {
    /// Authentication error (401)
    Auth,
    /// Format/validation error (400)
    Format,
    /// Rate limit exceeded (429)
    RateLimit,
    /// Billing/quota error (402/403)
    Billing,
    /// Request timeout
    Timeout,
    /// Unknown/other error
    Unknown,
}

impl AuthProfileFailureReason {
    /// Classify HTTP status code into failure reason
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::Format,
            401 => Self::Auth,
            402 | 403 => Self::Billing,
            429 => Self::RateLimit,
            408 | 504 => Self::Timeout,
            _ => Self::Unknown,
        }
    }
}

/// Per-profile usage statistics for round-robin and cooldown tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileUsageStats {
    /// Last successful use timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    /// Cooldown expiry for rate limit errors (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<u64>,
    /// Disabled expiry for billing errors (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_until: Option<u64>,
    /// Reason for being disabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<AuthProfileFailureReason>,
    /// Total error count (resets after failure window)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<u32>,
    /// Per-reason failure counts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_counts: Option<HashMap<AuthProfileFailureReason, u32>>,
    /// Last failure timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<u64>,
}

impl ProfileUsageStats {
    /// Get the timestamp when this profile becomes usable again
    pub fn unusable_until(&self) -> Option<u64> {
        let values: Vec<u64> = [self.cooldown_until, self.disabled_until]
            .into_iter()
            .flatten()
            .filter(|&v| v > 0)
            .collect();

        if values.is_empty() {
            None
        } else {
            Some(*values.iter().max().unwrap())
        }
    }

    /// Check if profile is currently in cooldown
    pub fn is_in_cooldown(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.unusable_until().is_some_and(|until| now < until)
    }
}

// ============================================================================
// Auth Profile Store
// ============================================================================

/// Persistent auth profile store
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfileStore {
    /// Store version for migrations
    pub version: u32,
    /// Profile ID -> Credential mapping
    pub profiles: HashMap<String, AuthProfileCredential>,
    /// Per-provider profile ordering override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<HashMap<String, Vec<String>>>,
    /// Last successfully used profile per provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_good: Option<HashMap<String, String>>,
    /// Usage statistics per profile
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_stats: Option<HashMap<String, ProfileUsageStats>>,
}

impl Default for AuthProfileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthProfileStore {
    /// Create an empty store
    pub fn new() -> Self {
        Self {
            version: AUTH_STORE_VERSION,
            profiles: HashMap::new(),
            order: None,
            last_good: None,
            usage_stats: None,
        }
    }

    /// List profile IDs for a given provider
    pub fn list_profiles_for_provider(&self, provider: &str) -> Vec<String> {
        let normalized = normalize_provider_id(provider);
        self.profiles
            .iter()
            .filter(|(_, cred)| normalize_provider_id(cred.provider()) == normalized)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get usage stats for a profile
    pub fn get_usage_stats(&self, profile_id: &str) -> Option<&ProfileUsageStats> {
        self.usage_stats.as_ref()?.get(profile_id)
    }

    /// Get mutable usage stats for a profile, creating if needed
    pub fn get_or_create_usage_stats(&mut self, profile_id: &str) -> &mut ProfileUsageStats {
        self.usage_stats
            .get_or_insert_with(HashMap::new)
            .entry(profile_id.to_string())
            .or_default()
    }

    /// Check if a profile is in cooldown
    pub fn is_profile_in_cooldown(&self, profile_id: &str) -> bool {
        self.get_usage_stats(profile_id)
            .is_some_and(|stats| stats.is_in_cooldown())
    }

    /// Add or update a profile
    pub fn upsert_profile(&mut self, profile_id: String, credential: AuthProfileCredential) {
        self.profiles.insert(profile_id, credential);
    }

    /// Remove a profile
    pub fn remove_profile(&mut self, profile_id: &str) -> Option<AuthProfileCredential> {
        let removed = self.profiles.remove(profile_id);
        if let Some(stats) = &mut self.usage_stats {
            stats.remove(profile_id);
        }
        if let Some(last_good) = &mut self.last_good {
            last_good.retain(|_, v| v != profile_id);
        }
        removed
    }
}

// ============================================================================
// Cooldown Algorithm
// ============================================================================

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
            billing_backoff: Duration::from_secs(5 * 60 * 60),  // 5 hours
            billing_max: Duration::from_secs(24 * 60 * 60),     // 24 hours
            failure_window: Duration::from_secs(24 * 60 * 60),  // 24 hours
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
/// - 2nd error: billing_backoff × 2 (10 hours)
/// - 3rd error: billing_backoff × 4 (20 hours)
/// - Max: billing_max (default 24 hours)
pub fn calculate_billing_cooldown_ms(error_count: u32, config: &CooldownConfig) -> u64 {
    let normalized = error_count.max(1);
    let exponent = (normalized - 1).min(10);
    let base_ms = config.billing_backoff.as_millis() as u64;
    let max_ms = config.billing_max.as_millis() as u64;

    (base_ms * 2u64.pow(exponent)).min(max_ms)
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

    let window_ms = config.failure_window.as_millis() as u64;
    let stats = store.get_or_create_usage_stats(profile_id);

    // Check if failure window expired (reset counters)
    let window_expired = stats.last_failure_at.is_some_and(|last| {
        last > 0 && now.saturating_sub(last) > window_ms
    });

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
    if let Some(stats) = store.usage_stats.as_mut().and_then(|s| s.get_mut(profile_id)) {
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

// ============================================================================
// Profile Ordering
// ============================================================================

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

// ============================================================================
// Utilities
// ============================================================================

/// Normalize provider ID for comparison (lowercase, trim)
pub fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_types() {
        let api_key = AuthProfileCredential::ApiKey(ApiKeyCredential {
            provider: "anthropic".to_string(),
            key: "sk-test-123".to_string(),
            email: None,
        });
        assert_eq!(api_key.provider(), "anthropic");
        assert_eq!(api_key.credential_type(), "api_key");
        assert!(api_key.is_valid());
        assert!(!api_key.is_expired());
        assert_eq!(api_key.type_score(), 2);

        let token = AuthProfileCredential::Token(TokenCredential {
            provider: "openai".to_string(),
            token: "tok-123".to_string(),
            expires: None,
            email: None,
        });
        assert_eq!(token.type_score(), 1);

        let oauth = AuthProfileCredential::OAuth(OAuthCredential {
            provider: "google".to_string(),
            access: "access-123".to_string(),
            refresh: Some("refresh-456".to_string()),
            expires: None,
            client_id: None,
            email: None,
        });
        assert_eq!(oauth.type_score(), 0);
    }

    #[test]
    fn test_credential_validation() {
        let empty_key = AuthProfileCredential::ApiKey(ApiKeyCredential {
            provider: "test".to_string(),
            key: "   ".to_string(),
            email: None,
        });
        assert!(!empty_key.is_valid());

        let expired_token = AuthProfileCredential::Token(TokenCredential {
            provider: "test".to_string(),
            token: "tok".to_string(),
            expires: Some(1000), // Ancient timestamp
            email: None,
        });
        assert!(expired_token.is_expired());
    }

    #[test]
    fn test_failure_reason_from_status() {
        assert_eq!(AuthProfileFailureReason::from_status(400), AuthProfileFailureReason::Format);
        assert_eq!(AuthProfileFailureReason::from_status(401), AuthProfileFailureReason::Auth);
        assert_eq!(AuthProfileFailureReason::from_status(402), AuthProfileFailureReason::Billing);
        assert_eq!(AuthProfileFailureReason::from_status(403), AuthProfileFailureReason::Billing);
        assert_eq!(AuthProfileFailureReason::from_status(429), AuthProfileFailureReason::RateLimit);
        assert_eq!(AuthProfileFailureReason::from_status(408), AuthProfileFailureReason::Timeout);
        assert_eq!(AuthProfileFailureReason::from_status(500), AuthProfileFailureReason::Unknown);
    }

    #[test]
    fn test_cooldown_calculation() {
        // Rate limit: 5^n minutes
        assert_eq!(calculate_cooldown_ms(1), 60_000);       // 1 min
        assert_eq!(calculate_cooldown_ms(2), 300_000);      // 5 min
        assert_eq!(calculate_cooldown_ms(3), 1_500_000);    // 25 min
        assert_eq!(calculate_cooldown_ms(4), 3_600_000);    // 1 hour (max)
        assert_eq!(calculate_cooldown_ms(10), 3_600_000);   // Still max
    }

    #[test]
    fn test_billing_cooldown_calculation() {
        let config = CooldownConfig::default();

        // Billing: 2^n × 5 hours
        let hour_ms = 60 * 60 * 1000u64;
        assert_eq!(calculate_billing_cooldown_ms(1, &config), 5 * hour_ms);   // 5h
        assert_eq!(calculate_billing_cooldown_ms(2, &config), 10 * hour_ms);  // 10h
        assert_eq!(calculate_billing_cooldown_ms(3, &config), 20 * hour_ms);  // 20h
        assert_eq!(calculate_billing_cooldown_ms(4, &config), 24 * hour_ms);  // 24h (max)
    }

    #[test]
    fn test_store_operations() {
        let mut store = AuthProfileStore::new();

        // Add profiles
        store.upsert_profile(
            "anthropic:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "anthropic".to_string(),
                key: "sk-123".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "anthropic:backup".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "anthropic".to_string(),
                key: "sk-456".to_string(),
                email: None,
            }),
        );

        let profiles = store.list_profiles_for_provider("anthropic");
        assert_eq!(profiles.len(), 2);

        // Remove profile
        store.remove_profile("anthropic:backup");
        let profiles = store.list_profiles_for_provider("anthropic");
        assert_eq!(profiles.len(), 1);
    }

    #[test]
    fn test_mark_profile_used() {
        let mut store = AuthProfileStore::new();
        store.upsert_profile(
            "test:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key".to_string(),
                email: None,
            }),
        );

        mark_profile_used(&mut store, "test:default");

        let stats = store.get_usage_stats("test:default").unwrap();
        assert!(stats.last_used.is_some());
        assert_eq!(stats.error_count, Some(0));
    }

    #[test]
    fn test_mark_profile_failure() {
        let mut store = AuthProfileStore::new();
        let config = CooldownConfig::default();

        store.upsert_profile(
            "test:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key".to_string(),
                email: None,
            }),
        );

        // First rate limit failure
        mark_profile_failure(&mut store, "test:default", AuthProfileFailureReason::RateLimit, &config);

        let stats = store.get_usage_stats("test:default").unwrap();
        assert_eq!(stats.error_count, Some(1));
        assert!(stats.cooldown_until.is_some());
        assert!(stats.is_in_cooldown());

        // Second failure
        mark_profile_failure(&mut store, "test:default", AuthProfileFailureReason::RateLimit, &config);

        let stats = store.get_usage_stats("test:default").unwrap();
        assert_eq!(stats.error_count, Some(2));
    }

    #[test]
    fn test_billing_failure_disabled() {
        let mut store = AuthProfileStore::new();
        let config = CooldownConfig::default();

        store.upsert_profile(
            "test:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key".to_string(),
                email: None,
            }),
        );

        mark_profile_failure(&mut store, "test:default", AuthProfileFailureReason::Billing, &config);

        let stats = store.get_usage_stats("test:default").unwrap();
        assert!(stats.disabled_until.is_some());
        assert_eq!(stats.disabled_reason, Some(AuthProfileFailureReason::Billing));
    }

    #[test]
    fn test_clear_cooldown() {
        let mut store = AuthProfileStore::new();
        let config = CooldownConfig::default();

        store.upsert_profile(
            "test:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key".to_string(),
                email: None,
            }),
        );

        mark_profile_failure(&mut store, "test:default", AuthProfileFailureReason::RateLimit, &config);
        assert!(store.is_profile_in_cooldown("test:default"));

        clear_profile_cooldown(&mut store, "test:default");
        assert!(!store.is_profile_in_cooldown("test:default"));
    }

    #[test]
    fn test_profile_ordering_by_type() {
        let mut store = AuthProfileStore::new();

        // Add profiles of different types
        store.upsert_profile(
            "test:api".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "test:token".to_string(),
            AuthProfileCredential::Token(TokenCredential {
                provider: "test".to_string(),
                token: "tok".to_string(),
                expires: None,
                email: None,
            }),
        );
        store.upsert_profile(
            "test:oauth".to_string(),
            AuthProfileCredential::OAuth(OAuthCredential {
                provider: "test".to_string(),
                access: "access".to_string(),
                refresh: None,
                expires: None,
                client_id: None,
                email: None,
            }),
        );

        let order = resolve_profile_order(&store, "test", None, None);

        // OAuth should be first, then token, then api_key
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], "test:oauth");
        assert_eq!(order[1], "test:token");
        assert_eq!(order[2], "test:api");
    }

    #[test]
    fn test_profile_ordering_round_robin() {
        let mut store = AuthProfileStore::new();

        // Add two API key profiles
        store.upsert_profile(
            "test:first".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key1".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "test:second".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key2".to_string(),
                email: None,
            }),
        );

        // Mark first as recently used
        mark_profile_used(&mut store, "test:first");

        let order = resolve_profile_order(&store, "test", None, None);

        // Second should be first (older/never used)
        assert_eq!(order[0], "test:second");
        assert_eq!(order[1], "test:first");
    }

    #[test]
    fn test_profile_ordering_cooldown_at_end() {
        let mut store = AuthProfileStore::new();
        let config = CooldownConfig::default();

        store.upsert_profile(
            "test:good".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key1".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "test:bad".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key2".to_string(),
                email: None,
            }),
        );

        // Put "bad" in cooldown
        mark_profile_failure(&mut store, "test:bad", AuthProfileFailureReason::RateLimit, &config);

        let order = resolve_profile_order(&store, "test", None, None);

        // Good should be first, bad at end
        assert_eq!(order[0], "test:good");
        assert_eq!(order[1], "test:bad");
    }

    #[test]
    fn test_profile_ordering_preferred() {
        let mut store = AuthProfileStore::new();

        store.upsert_profile(
            "test:a".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key1".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "test:b".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "test".to_string(),
                key: "key2".to_string(),
                email: None,
            }),
        );

        let order = resolve_profile_order(&store, "test", None, Some("test:b"));

        // Preferred should be first
        assert_eq!(order[0], "test:b");
    }

    #[test]
    fn test_normalize_provider_id() {
        assert_eq!(normalize_provider_id("Anthropic"), "anthropic");
        assert_eq!(normalize_provider_id("  OpenAI  "), "openai");
        assert_eq!(normalize_provider_id("google-gemini"), "google_gemini");
    }

    #[test]
    fn test_json_serialization() {
        let mut store = AuthProfileStore::new();
        store.upsert_profile(
            "anthropic:default".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "anthropic".to_string(),
                key: "sk-test".to_string(),
                email: Some("test@example.com".to_string()),
            }),
        );

        let json = serde_json::to_string_pretty(&store).unwrap();
        assert!(json.contains("\"type\": \"api_key\""));
        assert!(json.contains("\"provider\": \"anthropic\""));

        let deserialized: AuthProfileStore = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.profiles.len(), 1);
    }

    #[test]
    fn test_usage_stats_serialization() {
        let mut stats = ProfileUsageStats::default();
        stats.last_used = Some(1000);
        stats.cooldown_until = Some(2000);
        stats.error_count = Some(3);

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: ProfileUsageStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.last_used, Some(1000));
        assert_eq!(deserialized.cooldown_until, Some(2000));
        assert_eq!(deserialized.error_count, Some(3));
    }
}
