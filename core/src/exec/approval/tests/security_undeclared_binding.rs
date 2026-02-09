//! Security tests for undeclared parameter binding detection
//!
//! This module tests the sandbox's ability to detect when tools receive
//! parameters that don't match their declared bindings. This prevents
//! tools from accessing resources beyond their declared scope.

use crate::exec::approval::binding::check_binding_compliance;
use crate::exec::approval::types::EscalationReason;
use std::collections::HashMap;

/// Test fixed binding violation - exact value mismatch
#[test]
fn test_fixed_binding_violation() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/etc/passwd".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "Fixed binding violation should be detected"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding,
        "Should trigger UndeclaredBinding escalation"
    );
}

/// Test fixed binding match (control test)
#[test]
fn test_fixed_binding_match() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "Fixed binding match should not trigger escalation"
    );
}

/// Test pattern binding violation - file extension mismatch
#[test]
fn test_pattern_binding_violation() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/*.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/data.json".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "Pattern binding violation should be detected"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding,
        "Should trigger UndeclaredBinding escalation"
    );
}

/// Test pattern binding match (control test)
#[test]
fn test_pattern_binding_match() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/*.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "Pattern binding match should not trigger escalation"
    );
}

/// Test pattern binding with subdirectory match
#[test]
fn test_pattern_binding_subdirectory_match() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/**/*.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/subdir/output.txt".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "Pattern binding with subdirectory should match"
    );
}

/// Test range binding violation - port outside range
#[test]
fn test_range_binding_violation() {
    let declared_bindings = HashMap::from([
        ("port".to_string(), "8000-9000".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("port".to_string(), "80".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "Range binding violation should be detected"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding,
        "Should trigger UndeclaredBinding escalation"
    );
}

/// Test range binding match (control test)
#[test]
fn test_range_binding_match() {
    let declared_bindings = HashMap::from([
        ("port".to_string(), "8000-9000".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("port".to_string(), "8080".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "Range binding match should not trigger escalation"
    );
}

/// Test range binding at boundaries
#[test]
fn test_range_binding_boundaries() {
    let declared_bindings = HashMap::from([
        ("port".to_string(), "8000-9000".to_string()),
    ]);

    // Test lower boundary
    let runtime_params = HashMap::from([
        ("port".to_string(), "8000".to_string()),
    ]);
    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_ok(), "Lower boundary should be included");

    // Test upper boundary
    let runtime_params = HashMap::from([
        ("port".to_string(), "9000".to_string()),
    ]);
    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_ok(), "Upper boundary should be included");

    // Test below lower boundary
    let runtime_params = HashMap::from([
        ("port".to_string(), "7999".to_string()),
    ]);
    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_err(), "Below lower boundary should fail");

    // Test above upper boundary
    let runtime_params = HashMap::from([
        ("port".to_string(), "9001".to_string()),
    ]);
    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(result.is_err(), "Above upper boundary should fail");
}

/// Test missing required binding
#[test]
fn test_missing_required_binding() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);
    let runtime_params = HashMap::new(); // Empty params

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "Missing required binding should be detected"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding,
        "Should trigger UndeclaredBinding escalation"
    );
}

/// Test extra undeclared parameters
#[test]
fn test_extra_undeclared_parameters() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
        ("extra_param".to_string(), "value".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "Extra undeclared parameters should be detected"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding,
        "Should trigger UndeclaredBinding escalation"
    );
}

/// Test multiple bindings - all match
#[test]
fn test_multiple_bindings_all_match() {
    let declared_bindings = HashMap::from([
        ("input_path".to_string(), "/tmp/*.txt".to_string()),
        ("output_path".to_string(), "/tmp/output.txt".to_string()),
        ("port".to_string(), "8000-9000".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("input_path".to_string(), "/tmp/input.txt".to_string()),
        ("output_path".to_string(), "/tmp/output.txt".to_string()),
        ("port".to_string(), "8080".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "All matching bindings should not trigger escalation"
    );
}

/// Test multiple bindings - one violation
#[test]
fn test_multiple_bindings_one_violation() {
    let declared_bindings = HashMap::from([
        ("input_path".to_string(), "/tmp/*.txt".to_string()),
        ("output_path".to_string(), "/tmp/output.txt".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("input_path".to_string(), "/tmp/input.txt".to_string()),
        ("output_path".to_string(), "/etc/passwd".to_string()), // Violation
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_err(),
        "One binding violation should trigger escalation"
    );
    assert_eq!(
        result.unwrap_err().reason,
        EscalationReason::UndeclaredBinding
    );
}

/// Test empty bindings and empty params (control test)
#[test]
fn test_empty_bindings_and_params() {
    let declared_bindings = HashMap::new();
    let runtime_params = HashMap::new();

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    assert!(
        result.is_ok(),
        "Empty bindings and params should not trigger escalation"
    );
}

/// Test case sensitivity in pattern matching
#[test]
fn test_pattern_case_sensitivity() {
    let declared_bindings = HashMap::from([
        ("file_path".to_string(), "/tmp/*.TXT".to_string()),
    ]);
    let runtime_params = HashMap::from([
        ("file_path".to_string(), "/tmp/output.txt".to_string()),
    ]);

    let result = check_binding_compliance(&runtime_params, &declared_bindings);
    // Pattern matching should be case-sensitive by default
    assert!(
        result.is_err(),
        "Case mismatch in pattern should trigger escalation"
    );
}
