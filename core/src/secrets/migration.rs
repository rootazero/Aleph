//! Config migration: plaintext api_key → SecretVault
//!
//! Detects plaintext api_key fields in config.toml and migrates them
//! to the encrypted vault, replacing with secret_name references.

use std::path::Path;
use tracing::{info, warn};

use super::types::{EntryMetadata, SecretError};
use super::vault::SecretVault;
use crate::config::Config;

/// Result of a migration attempt.
#[derive(Debug)]
pub struct MigrationResult {
    /// Number of keys migrated.
    pub migrated_count: usize,
    /// Names of migrated providers.
    pub migrated_providers: Vec<String>,
}

/// Check if config has any plaintext api_key fields that need migration.
pub fn needs_migration(config: &Config) -> bool {
    config.providers.values().any(|p| p.api_key.is_some())
}

/// Migrate all plaintext api_key fields to the vault.
///
/// For each provider with a plaintext api_key:
/// 1. Store the key in the vault as "{provider_name}_api_key"
/// 2. Set secret_name on the provider config
/// 3. Clear the api_key field
///
/// Returns the list of migrated provider names.
pub fn migrate_api_keys(
    config: &mut Config,
    vault: &mut SecretVault,
) -> Result<MigrationResult, SecretError> {
    let mut migrated = Vec::new();

    // Collect providers that need migration
    let to_migrate: Vec<(String, String)> = config
        .providers
        .iter()
        .filter_map(|(name, p)| p.api_key.as_ref().map(|key| (name.clone(), key.clone())))
        .collect();

    for (provider_name, api_key) in to_migrate {
        let secret_name = format!("{}_api_key", provider_name.replace('-', "_"));

        // Store in vault
        vault.set(
            &secret_name,
            &api_key,
            EntryMetadata {
                description: Some(format!("API key for provider '{}'", provider_name)),
                provider: Some(provider_name.clone()),
            },
        )?;

        // Update config: clear api_key, set secret_name
        if let Some(provider) = config.providers.get_mut(&provider_name) {
            provider.api_key = None;
            provider.secret_name = Some(secret_name.clone());
        }

        info!(
            provider = %provider_name,
            secret_name = %secret_name,
            "Migrated API key to vault"
        );
        migrated.push(provider_name);
    }

    Ok(MigrationResult {
        migrated_count: migrated.len(),
        migrated_providers: migrated,
    })
}

/// Rewrite config.toml after migration to remove plaintext keys.
///
/// Uses incremental save to only update the providers section.
pub fn save_migrated_config(config: &Config) -> Result<(), SecretError> {
    let path = Config::default_path();
    if path.exists() {
        config
            .save_incremental(&["providers"])
            .map_err(|e| SecretError::MigrationFailed {
                provider: "all".into(),
                reason: format!("Failed to save migrated config: {}", e),
            })?;
        info!("Config file updated: plaintext API keys removed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use tempfile::TempDir;

    fn make_config_with_plaintext_keys() -> Config {
        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("claude-sonnet-4-20250514");
        provider.api_key = Some("sk-ant-test-key-123".to_string());
        provider.protocol = Some("anthropic".to_string());
        config.providers.insert("claude-main".to_string(), provider);

        let mut provider2 = ProviderConfig::test_config("gpt-4o");
        provider2.api_key = Some("sk-openai-test-456".to_string());
        config
            .providers
            .insert("openai-main".to_string(), provider2);

        config
    }

    #[test]
    fn test_needs_migration_with_plaintext() {
        let config = make_config_with_plaintext_keys();
        assert!(needs_migration(&config));
    }

    #[test]
    fn test_needs_migration_without_plaintext() {
        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("model");
        provider.api_key = None;
        provider.secret_name = Some("my_key".into());
        config.providers.insert("test".into(), provider);

        assert!(!needs_migration(&config));
    }

    #[test]
    fn test_migrate_api_keys() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();
        let mut config = make_config_with_plaintext_keys();

        let result = migrate_api_keys(&mut config, &mut vault).unwrap();

        // Check migration counts
        assert_eq!(result.migrated_count, 2);
        assert!(result
            .migrated_providers
            .contains(&"claude-main".to_string()));
        assert!(result
            .migrated_providers
            .contains(&"openai-main".to_string()));

        // Check config updated
        let claude = config.providers.get("claude-main").unwrap();
        assert!(claude.api_key.is_none());
        assert_eq!(claude.secret_name.as_deref(), Some("claude_main_api_key"));

        let openai = config.providers.get("openai-main").unwrap();
        assert!(openai.api_key.is_none());
        assert_eq!(openai.secret_name.as_deref(), Some("openai_main_api_key"));

        // Check vault has the keys
        let secret = vault.get("claude_main_api_key").unwrap();
        assert_eq!(secret.expose(), "sk-ant-test-key-123");

        let secret = vault.get("openai_main_api_key").unwrap();
        assert_eq!(secret.expose(), "sk-openai-test-456");
    }

    #[test]
    fn test_migrate_skips_already_migrated() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();

        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("model");
        provider.api_key = None;
        provider.secret_name = Some("existing_key".into());
        config.providers.insert("test".into(), provider);

        let result = migrate_api_keys(&mut config, &mut vault).unwrap();
        assert_eq!(result.migrated_count, 0);
    }
}
