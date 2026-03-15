//! Auth profile store — persistent storage for credentials and usage stats.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::credentials::AuthProfileCredential;
use super::failure::ProfileUsageStats;
use super::{normalize_provider_id, AUTH_STORE_VERSION};

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
