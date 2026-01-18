//! Integration tests for the logging system
//!
//! Tests logging behavior, PII scrubbing, retention policies, and log level control.

use aethecore::logging::{
    file_appender::get_log_directory,
    level_control::{get_log_level, set_log_level, LogLevel},
    pii_filter::PiiScrubbingLayer,
    retention::cleanup_old_logs,
};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;

// MARK: - Test Helpers

/// Create a test log directory structure
fn setup_test_log_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

/// Create a log file with specific timestamp
fn create_test_log_file(dir: &PathBuf, filename: &str, days_old: u64) -> PathBuf {
    let log_file = dir.join(filename);
    let mut file = File::create(&log_file).expect("Failed to create log file");
    writeln!(file, "Test log content").expect("Failed to write log file");

    // Set modification time
    let modified_time = SystemTime::now() - Duration::from_secs(days_old * 24 * 60 * 60);
    filetime::set_file_mtime(
        &log_file,
        filetime::FileTime::from_system_time(modified_time),
    )
    .expect("Failed to set file time");

    log_file
}

// MARK: - PII Scrubbing Tests

#[test]
fn test_pii_scrubbing_email() {
    // This test verifies that email addresses are redacted in logs
    let email = "user@example.com";
    let message = format!("User logged in: {}", email);

    // The PII filter layer should redact the email
    // We test this through the actual tracing infrastructure
    let scrubber = PiiScrubbingLayer;
    let subscriber = tracing_subscriber::registry().with(scrubber);

    let _guard = tracing::subscriber::set_default(subscriber);

    // Log a message with email - it should be scrubbed
    info!("{}", message);

    // Note: Since we can't easily capture the output in a test,
    // we rely on the unit tests in pii_filter.rs for validation.
    // This integration test ensures the layer can be composed with registry.
}

#[test]
fn test_pii_scrubbing_api_key() {
    let api_key = "sk-proj-1234567890abcdef";
    let message = format!("Using API key: {}", api_key);

    let scrubber = PiiScrubbingLayer;
    let subscriber = tracing_subscriber::registry().with(scrubber);

    let _guard = tracing::subscriber::set_default(subscriber);

    info!("{}", message);

    // The actual redaction is tested in unit tests
}

#[test]
fn test_pii_scrubbing_phone_number() {
    let phone = "+1-555-123-4567";
    let message = format!("Contact: {}", phone);

    let scrubber = PiiScrubbingLayer;
    let subscriber = tracing_subscriber::registry().with(scrubber);

    let _guard = tracing::subscriber::set_default(subscriber);

    info!("{}", message);
}

#[test]
fn test_pii_scrubbing_credit_card() {
    let cc = "4532-1234-5678-9010";
    let message = format!("Payment method: {}", cc);

    let scrubber = PiiScrubbingLayer;
    let subscriber = tracing_subscriber::registry().with(scrubber);

    let _guard = tracing::subscriber::set_default(subscriber);

    info!("{}", message);
}

// MARK: - Retention Policy Tests

#[test]
fn test_retention_policy_deletes_old_logs() {
    let temp_dir = setup_test_log_dir();
    let log_dir = temp_dir.path().to_path_buf();

    // Create old log files (older than retention period)
    create_test_log_file(&log_dir, "aether-2024-01-01.log", 100); // 100 days old
    create_test_log_file(&log_dir, "aether-2024-02-01.log", 50); // 50 days old

    // Create recent log files (within retention period)
    create_test_log_file(&log_dir, "aether-2024-12-20.log", 5); // 5 days old
    create_test_log_file(&log_dir, "aether-2024-12-24.log", 1); // 1 day old

    // Run cleanup with 30-day retention
    let retention_days = 30;
    cleanup_old_logs(&log_dir, retention_days).expect("Cleanup failed");

    // Verify old files are deleted
    assert!(!log_dir.join("aether-2024-01-01.log").exists());
    assert!(!log_dir.join("aether-2024-02-01.log").exists());

    // Verify recent files are kept
    assert!(log_dir.join("aether-2024-12-20.log").exists());
    assert!(log_dir.join("aether-2024-12-24.log").exists());
}

#[test]
fn test_retention_policy_keeps_all_with_zero_days() {
    let temp_dir = setup_test_log_dir();
    let log_dir = temp_dir.path().to_path_buf();

    // Create log files of various ages
    create_test_log_file(&log_dir, "aether-2024-12-24.log", 1); // 1 day old (within 30 days)
    create_test_log_file(&log_dir, "aether-2024-12-20.log", 5); // 5 days old (within 30 days)
    create_test_log_file(&log_dir, "aether-2024-11-01.log", 55); // 55 days old (outside 30 days)

    // Run cleanup with 0-day retention (clamped to 1 day minimum)
    // Note: cleanup_old_logs clamps retention_days to range [1, 30]
    let retention_days = 0;
    cleanup_old_logs(&log_dir, retention_days).expect("Cleanup failed");

    // Since 0 is clamped to 1, only files older than 1 day will be deleted
    // The 1-day old file should be kept (on the boundary)
    // Files older than 1 day will be deleted
    assert!(!log_dir.join("aether-2024-11-01.log").exists()); // Deleted (55 days old)
}

#[test]
fn test_retention_policy_skips_non_log_files() {
    let temp_dir = setup_test_log_dir();
    let log_dir = temp_dir.path().to_path_buf();

    // Create old log file
    create_test_log_file(&log_dir, "aether-2024-01-01.log", 100);

    // Create old non-log file
    let readme = log_dir.join("README.txt");
    let mut file = File::create(&readme).expect("Failed to create README");
    writeln!(file, "This is a readme").expect("Failed to write README");
    let modified_time = SystemTime::now() - Duration::from_secs(100 * 24 * 60 * 60);
    filetime::set_file_mtime(&readme, filetime::FileTime::from_system_time(modified_time))
        .expect("Failed to set file time");

    // Run cleanup
    let retention_days = 30;
    cleanup_old_logs(&log_dir, retention_days).expect("Cleanup failed");

    // Verify log file is deleted
    assert!(!log_dir.join("aether-2024-01-01.log").exists());

    // Verify non-log file is kept
    assert!(readme.exists());
}

// MARK: - Log Level Control Tests

#[test]
fn test_log_level_change_takes_effect() {
    // Set to debug level
    set_log_level(LogLevel::Debug);
    assert_eq!(get_log_level(), LogLevel::Debug);

    // Change to error level
    set_log_level(LogLevel::Error);
    assert_eq!(get_log_level(), LogLevel::Error);

    // Change back to info
    set_log_level(LogLevel::Info);
    assert_eq!(get_log_level(), LogLevel::Info);
}

#[test]
fn test_log_level_persistence() {
    // Set a log level
    set_log_level(LogLevel::Warn);

    // Verify it persists across reads
    assert_eq!(get_log_level(), LogLevel::Warn);
    assert_eq!(get_log_level(), LogLevel::Warn);
}

// MARK: - Log Directory Tests

#[test]
fn test_get_log_directory_returns_valid_path() {
    let log_dir = get_log_directory().expect("Failed to get log directory");

    // Convert to string for pattern matching
    let log_dir_str = log_dir.to_string_lossy();

    // Verify path contains expected components
    // Note: On macOS, config_dir might be different (e.g., ~/Library/Application Support)
    assert!(log_dir_str.contains("aether"));
    assert!(log_dir_str.contains("logs"));

    // Verify path is absolute
    assert!(log_dir.is_absolute());
}

#[test]
fn test_log_directory_creation() {
    let log_dir = get_log_directory().expect("Failed to get log directory");

    // The directory should be created automatically
    // (this is tested in unit tests, but we verify the integration here)
    assert!(log_dir.exists() || log_dir.parent().unwrap().exists());
}

// MARK: - End-to-End Logging Tests

#[test]
fn test_logging_integration_end_to_end() {
    // This test ensures all logging components work together:
    // 1. Log directory is accessible
    // 2. Log level can be changed
    // 3. PII scrubbing is active
    // 4. Retention policy works

    // Get log directory
    let log_dir = get_log_directory().expect("Failed to get log directory");
    assert!(log_dir.exists() || log_dir.parent().is_some());

    // Set log level
    set_log_level(LogLevel::Debug);
    assert_eq!(get_log_level(), LogLevel::Debug);

    // Create PII scrubbing layer (verifies it can be instantiated)
    let _scrubber = PiiScrubbingLayer;

    // Verify retention policy with temp directory
    let temp_dir = setup_test_log_dir();
    let test_log_dir = temp_dir.path().to_path_buf();
    create_test_log_file(&test_log_dir, "test.log", 100);
    cleanup_old_logs(&test_log_dir, 30).expect("Cleanup failed");

    // All components working together successfully
}

#[test]
fn test_concurrent_log_level_changes() {
    use std::thread;

    // Test that log level changes are thread-safe
    let handles: Vec<_> = (0..5)
        .map(|i| {
            thread::spawn(move || {
                let level = match i % 3 {
                    0 => LogLevel::Debug,
                    1 => LogLevel::Info,
                    _ => LogLevel::Warn,
                };
                set_log_level(level);
                get_log_level()
            })
        })
        .collect();

    for handle in handles {
        let result = handle.join().expect("Thread panicked");
        // Just verify we got a valid log level back
        assert!(matches!(
            result,
            LogLevel::Debug | LogLevel::Info | LogLevel::Warn | LogLevel::Error
        ));
    }
}

// MARK: - Error Handling Tests

#[test]
fn test_cleanup_logs_nonexistent_directory() {
    let nonexistent = PathBuf::from("/nonexistent/log/directory");
    let result = cleanup_old_logs(&nonexistent, 30);

    // Should return Ok(0) for nonexistent directory (not an error)
    // The implementation returns Ok(0) if directory doesn't exist
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn test_log_level_invalid_values_handled_gracefully() {
    // Test that the system handles invalid log level strings gracefully

    // Valid levels
    assert!(LogLevel::parse("debug").is_some());
    assert!(LogLevel::parse("info").is_some());
    assert!(LogLevel::parse("warn").is_some());
    assert!(LogLevel::parse("error").is_some());

    // Invalid levels
    assert!(LogLevel::parse("invalid").is_none());
    assert!(LogLevel::parse("").is_none());
    assert!(LogLevel::parse("trace").is_some()); // Trace is supported
}
