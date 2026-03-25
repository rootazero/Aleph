//! Auth profile management for API key rotation.
//!
//! Provides:
//! - Multiple credential types (API key, token, OAuth)
//! - Per-profile usage tracking with cooldown support
//! - Round-robin profile ordering with type preference
//! - Exponential backoff for rate limits and billing errors
//!
//! Reference: Moltbot src/agents/auth-profiles/

mod cooldown;
mod credentials;
mod failure;
mod ordering;
mod store;

// Re-export all public API items
pub use cooldown::{
    calculate_billing_cooldown_ms, calculate_cooldown_ms, clear_profile_cooldown,
    mark_profile_failure, mark_profile_good, mark_profile_used, CooldownConfig,
};
pub use credentials::{
    ApiKeyCredential, AuthProfileCredential, OAuthCredential, TokenCredential,
};
pub use failure::{AuthProfileFailureReason, ProfileUsageStats};
pub use ordering::resolve_profile_order;
pub use store::AuthProfileStore;

/// Current store version for migrations
pub const AUTH_STORE_VERSION: u32 = 1;

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
            client_secret: None,
            token_endpoint: None,
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
        assert_eq!(
            AuthProfileFailureReason::from_status(400),
            AuthProfileFailureReason::Format
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(401),
            AuthProfileFailureReason::Auth
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(402),
            AuthProfileFailureReason::Billing
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(403),
            AuthProfileFailureReason::Billing
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(429),
            AuthProfileFailureReason::RateLimit
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(408),
            AuthProfileFailureReason::Timeout
        );
        assert_eq!(
            AuthProfileFailureReason::from_status(500),
            AuthProfileFailureReason::Unknown
        );
    }

    #[test]
    fn test_cooldown_calculation() {
        // Rate limit: 5^n minutes
        assert_eq!(calculate_cooldown_ms(1), 60_000); // 1 min
        assert_eq!(calculate_cooldown_ms(2), 300_000); // 5 min
        assert_eq!(calculate_cooldown_ms(3), 1_500_000); // 25 min
        assert_eq!(calculate_cooldown_ms(4), 3_600_000); // 1 hour (max)
        assert_eq!(calculate_cooldown_ms(10), 3_600_000); // Still max
    }

    #[test]
    fn test_billing_cooldown_calculation() {
        let config = CooldownConfig::default();

        // Billing: 2^n × 5 hours
        let hour_ms = 60 * 60 * 1000u64;
        assert_eq!(calculate_billing_cooldown_ms(1, &config), 5 * hour_ms); // 5h
        assert_eq!(calculate_billing_cooldown_ms(2, &config), 10 * hour_ms); // 10h
        assert_eq!(calculate_billing_cooldown_ms(3, &config), 20 * hour_ms); // 20h
        assert_eq!(calculate_billing_cooldown_ms(4, &config), 24 * hour_ms); // 24h (max)
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
        mark_profile_failure(
            &mut store,
            "test:default",
            AuthProfileFailureReason::RateLimit,
            &config,
        );

        let stats = store.get_usage_stats("test:default").unwrap();
        assert_eq!(stats.error_count, Some(1));
        assert!(stats.cooldown_until.is_some());
        assert!(stats.is_in_cooldown());

        // Second failure
        mark_profile_failure(
            &mut store,
            "test:default",
            AuthProfileFailureReason::RateLimit,
            &config,
        );

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

        mark_profile_failure(
            &mut store,
            "test:default",
            AuthProfileFailureReason::Billing,
            &config,
        );

        let stats = store.get_usage_stats("test:default").unwrap();
        assert!(stats.disabled_until.is_some());
        assert_eq!(
            stats.disabled_reason,
            Some(AuthProfileFailureReason::Billing)
        );
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

        mark_profile_failure(
            &mut store,
            "test:default",
            AuthProfileFailureReason::RateLimit,
            &config,
        );
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
                client_secret: None,
                token_endpoint: None,
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
        mark_profile_failure(
            &mut store,
            "test:bad",
            AuthProfileFailureReason::RateLimit,
            &config,
        );

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
        let stats = ProfileUsageStats {
            last_used: Some(1000),
            cooldown_until: Some(2000),
            error_count: Some(3),
            ..ProfileUsageStats::default()
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: ProfileUsageStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.last_used, Some(1000));
        assert_eq!(deserialized.cooldown_until, Some(2000));
        assert_eq!(deserialized.error_count, Some(3));
    }
}
