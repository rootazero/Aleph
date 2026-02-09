//! Security tests for sensitive directory access detection
//!
//! This module tests the sandbox's ability to detect and escalate when tools
//! attempt to access sensitive directories (SSH keys, GPG keys, credentials, etc.)
//! even when those paths are within the approved scope.

use crate::exec::approval::escalation::{check_path_escalation, is_sensitive_directory};
use crate::exec::approval::types::EscalationReason;
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// Unit Tests: is_sensitive_directory()
// ============================================================================

/// Test SSH key directory detection
#[test]
fn test_ssh_key_detection() {
    let path = PathBuf::from("/Users/test/.ssh/id_rsa");
    assert!(
        is_sensitive_directory(&path),
        "SSH private key should be detected as sensitive"
    );
}

/// Test SSH config file detection
#[test]
fn test_ssh_config_detection() {
    let path = PathBuf::from("/Users/test/.ssh/config");
    assert!(
        is_sensitive_directory(&path),
        "SSH config should be detected as sensitive"
    );
}

/// Test nested SSH directory detection
#[test]
fn test_nested_ssh_directory() {
    let path = PathBuf::from("/Users/test/.ssh/config.d/work.conf");
    assert!(
        is_sensitive_directory(&path),
        "Nested SSH config should be detected as sensitive"
    );
}

/// Test GPG key directory detection
#[test]
fn test_gpg_key_detection() {
    let path = PathBuf::from("/Users/test/.gnupg/private-keys-v1.d/key.gpg");
    assert!(
        is_sensitive_directory(&path),
        "GPG private key should be detected as sensitive"
    );
}

/// Test GPG trustdb detection
#[test]
fn test_gpg_trustdb_detection() {
    let path = PathBuf::from("/Users/test/.gnupg/trustdb.gpg");
    assert!(
        is_sensitive_directory(&path),
        "GPG trustdb should be detected as sensitive"
    );
}

/// Test AWS credentials detection
#[test]
fn test_aws_credentials_detection() {
    let path = PathBuf::from("/Users/test/.aws/credentials");
    assert!(
        is_sensitive_directory(&path),
        "AWS credentials should be detected as sensitive"
    );
}

/// Test AWS config detection
#[test]
fn test_aws_config_detection() {
    let path = PathBuf::from("/Users/test/.aws/config");
    assert!(
        is_sensitive_directory(&path),
        "AWS config should be detected as sensitive"
    );
}

/// Test Google Cloud credentials detection
#[test]
fn test_gcloud_credentials_detection() {
    let path = PathBuf::from("/Users/test/.config/gcloud/credentials.db");
    assert!(
        is_sensitive_directory(&path),
        "Google Cloud credentials should be detected as sensitive"
    );
}

/// Test Google Cloud application default credentials
#[test]
fn test_gcloud_adc_detection() {
    let path = PathBuf::from("/Users/test/.config/gcloud/application_default_credentials.json");
    assert!(
        is_sensitive_directory(&path),
        "Google Cloud ADC should be detected as sensitive"
    );
}

/// Test macOS Keychain detection
#[test]
fn test_keychain_detection() {
    let path = PathBuf::from("/Applications/Keychain.app/Contents/MacOS/Keychain");
    assert!(
        is_sensitive_directory(&path),
        "macOS Keychain should be detected as sensitive"
    );
}

/// Test non-sensitive path (control test)
#[test]
fn test_non_sensitive_path() {
    let path = PathBuf::from("/Users/test/Documents/file.txt");
    assert!(
        !is_sensitive_directory(&path),
        "Regular document should not be detected as sensitive"
    );
}

/// Test non-sensitive path in home directory
#[test]
fn test_non_sensitive_home_path() {
    let path = PathBuf::from("/Users/test/Downloads/image.png");
    assert!(
        !is_sensitive_directory(&path),
        "Regular download should not be detected as sensitive"
    );
}

/// Test path with similar name but not sensitive
#[test]
fn test_false_positive_ssh_name() {
    let path = PathBuf::from("/Users/test/projects/ssh-client/main.rs");
    assert!(
        !is_sensitive_directory(&path),
        "File with 'ssh' in name but not in .ssh directory should not be sensitive"
    );
}

// ============================================================================
// Integration Tests: check_path_escalation() with Sensitive Directories
// ============================================================================

/// Test escalation when accessing SSH key within approved scope
#[test]
fn test_escalation_ssh_key_in_scope() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/.ssh/id_rsa".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "SSH key access should trigger escalation even within approved scope"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory,
        "Should trigger SensitiveDirectory escalation"
    );
}

/// Test escalation when accessing GPG key within approved scope
#[test]
fn test_escalation_gpg_key_in_scope() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/.gnupg/private-keys-v1.d/key.gpg".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "GPG key access should trigger escalation even within approved scope"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

/// Test escalation when accessing AWS credentials within approved scope
#[test]
fn test_escalation_aws_credentials_in_scope() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/.aws/credentials".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "AWS credentials access should trigger escalation even within approved scope"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

/// Test escalation when accessing Google Cloud credentials within approved scope
#[test]
fn test_escalation_gcloud_credentials_in_scope() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/.config/gcloud/credentials.db".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "Google Cloud credentials access should trigger escalation even within approved scope"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

/// Test escalation when accessing nested sensitive path
#[test]
fn test_escalation_nested_sensitive_path() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/.ssh/config.d/work.conf".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "Nested SSH config access should trigger escalation"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

/// Test no escalation for non-sensitive path within approved scope
#[test]
fn test_no_escalation_non_sensitive_in_scope() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("file_path".to_string(), "/Users/test/Documents/file.txt".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_none(),
        "Non-sensitive path within approved scope should not trigger escalation"
    );
}

/// Test escalation with multiple parameters, one sensitive
#[test]
fn test_escalation_multiple_params_one_sensitive() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("input_file".to_string(), "/Users/test/input.txt".to_string());
    params.insert("output_file".to_string(), "/Users/test/.ssh/id_rsa".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "Should trigger escalation when any parameter accesses sensitive directory"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

/// Test escalation with directory parameter
#[test]
fn test_escalation_directory_parameter() {
    let approved_paths = vec!["/Users/test/*".to_string()];
    let mut params = HashMap::new();
    params.insert("target_dir".to_string(), "/Users/test/.ssh".to_string());

    let trigger = check_path_escalation(&params, &approved_paths);
    assert!(
        trigger.is_some(),
        "Should trigger escalation when accessing sensitive directory"
    );
    assert_eq!(
        trigger.unwrap().reason,
        EscalationReason::SensitiveDirectory
    );
}

// ============================================================================
// Edge Cases and Platform-Specific Tests
// ============================================================================

/// Test case sensitivity handling (macOS is case-insensitive by default)
#[test]
fn test_case_sensitivity_ssh() {
    // Note: Current implementation is case-sensitive
    // This test documents the expected behavior
    let path_lower = PathBuf::from("/Users/test/.ssh/id_rsa");
    let path_upper = PathBuf::from("/Users/test/.SSH/id_rsa");

    assert!(
        is_sensitive_directory(&path_lower),
        "Lowercase .ssh should be detected"
    );

    // Current implementation is case-sensitive
    // On macOS, filesystem is case-insensitive but case-preserving
    // This test documents that we may want to enhance this in the future
    assert!(
        !is_sensitive_directory(&path_upper),
        "Current implementation is case-sensitive (may need enhancement for macOS)"
    );
}

/// Test absolute path vs relative path handling
#[test]
fn test_relative_path_sensitive() {
    let path = PathBuf::from(".ssh/id_rsa");
    // Relative paths should still be detected if they contain sensitive patterns
    assert!(
        is_sensitive_directory(&path),
        "Relative path to sensitive directory should be detected"
    );
}

/// Test symlink-like path (path component contains sensitive pattern)
#[test]
fn test_symlink_like_path() {
    let path = PathBuf::from("/Users/test/backup/.ssh/id_rsa");
    assert!(
        is_sensitive_directory(&path),
        "Path containing .ssh directory should be detected"
    );
}

/// Test root-level sensitive directories
#[test]
fn test_root_level_ssh() {
    let path = PathBuf::from("/root/.ssh/id_rsa");
    assert!(
        is_sensitive_directory(&path),
        "Root user SSH key should be detected"
    );
}

/// Test system-level sensitive directories
#[test]
fn test_system_level_keychain() {
    let path = PathBuf::from("/System/Library/Keychain.app/Contents/Resources/data");
    assert!(
        is_sensitive_directory(&path),
        "System-level Keychain should be detected"
    );
}


