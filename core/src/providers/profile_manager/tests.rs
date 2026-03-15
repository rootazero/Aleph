use super::*;
use crate::providers::auth_profiles::AuthProfileFailureReason;
use crate::providers::profile_config::ProfileTier;
use tempfile::TempDir;

fn create_test_config(temp_dir: &TempDir) -> std::path::PathBuf {
    let config_path = temp_dir.path().join("profiles.toml");
    let content = r#"
        [profiles.anthropic_primary]
        provider = "anthropic"
        api_key = "sk-ant-primary"
        tier = "primary"

        [profiles.anthropic_backup]
        provider = "anthropic"
        api_key = "sk-ant-backup"
        tier = "backup"

        [profiles.openai_main]
        provider = "openai"
        api_key = "sk-openai-main"
        tier = "primary"
    "#;
    std::fs::write(&config_path, content).unwrap();
    config_path
}

#[test]
fn test_manager_creation() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();
    assert_eq!(manager.profile_count(), 3);
}

#[test]
fn test_get_available_profile() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Should get primary profile first
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.provider, "anthropic");
    assert_eq!(profile.tier, ProfileTier::Primary);
    assert_eq!(profile.api_key, "sk-ant-primary");
}

#[test]
fn test_mark_failure_triggers_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Mark primary as failed
    manager
        .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Check that profile is in cooldown
    let profiles = manager.profiles_for_provider("anthropic");
    let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
    assert!(primary.in_cooldown);
    assert!(primary.cooldown_remaining_ms.is_some());
    assert_eq!(primary.failure_count, 1);
}

#[test]
fn test_fallback_to_backup_on_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Mark primary as failed
    manager
        .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Should get backup profile now
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.id, "anthropic_backup");
    assert_eq!(profile.tier, ProfileTier::Backup);
}

#[test]
fn test_mark_success_clears_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Mark as failed, then success
    manager
        .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
        .unwrap();
    manager.mark_success("anthropic_primary").unwrap();

    // Should not be in cooldown
    let profiles = manager.profiles_for_provider("anthropic");
    let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
    assert!(!primary.in_cooldown);
    assert_eq!(primary.failure_count, 0);
}

#[test]
fn test_record_usage() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir.clone()).unwrap();

    // Record usage
    manager
        .record_usage("main", "anthropic_primary", 1000, 500, 0.015)
        .unwrap();

    // Check that state was saved
    let state_path = agents_dir.join("main").join("state.json");
    assert!(state_path.exists());

    let state = AgentState::load(&state_path).unwrap();
    let usage = state.get_usage("anthropic_primary").unwrap();
    assert_eq!(usage.input_tokens, 1000);
    assert_eq!(usage.output_tokens, 500);
    assert!((usage.total_cost_usd - 0.015).abs() < 0.0001);
    assert_eq!(usage.request_count, 1);
}

#[test]
fn test_budget_override() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Set a $10 budget
    manager
        .set_budget_override("main", "anthropic_primary", Some(10.0))
        .unwrap();

    // Record usage that exceeds budget
    manager
        .record_usage("main", "anthropic_primary", 100000, 50000, 11.0)
        .unwrap();

    // Should skip to backup because primary exceeds budget
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.id, "anthropic_backup");
}

#[test]
fn test_disable_profile_for_agent() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Disable primary for this agent
    manager
        .disable_profile_for_agent("main", "anthropic_primary", true)
        .unwrap();

    // Should get backup profile
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.id, "anthropic_backup");
}

#[test]
fn test_list_profiles() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    let profiles = manager.list_profiles();
    assert_eq!(profiles.len(), 3);

    // All should have resolvable keys (literal keys)
    assert!(profiles.iter().all(|p| p.key_resolvable));
    assert!(profiles.iter().all(|p| !p.uses_env_var));
}

#[test]
fn test_no_profiles_error() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    // Empty config
    std::fs::write(&config_path, "").unwrap();

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    let result = manager.get_available_profile("anthropic", "main");
    assert!(matches!(
        result,
        Err(ProfileManagerError::NoProfilesAvailable(_))
    ));
}

#[test]
fn test_clear_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Mark as failed
    manager
        .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Clear cooldown
    manager.clear_cooldown("anthropic_primary").unwrap();

    // Should not be in cooldown
    let profiles = manager.profiles_for_provider("anthropic");
    let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
    assert!(!primary.in_cooldown);
}

#[test]
fn test_reload_config() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = create_test_config(&temp_dir);
    let agents_dir = temp_dir.path().join("agents");

    let manager = AuthProfileManager::with_paths(config_path.clone(), agents_dir).unwrap();
    assert_eq!(manager.profile_count(), 3);

    // Add a new profile to config
    let new_content = r#"
        [profiles.anthropic_primary]
        provider = "anthropic"
        api_key = "sk-ant-primary"
        tier = "primary"

        [profiles.new_profile]
        provider = "gemini"
        api_key = "gemini-key"
        tier = "primary"
    "#;
    std::fs::write(&config_path, new_content).unwrap();

    // Reload
    manager.reload_config().unwrap();
    assert_eq!(manager.profile_count(), 2);
}
