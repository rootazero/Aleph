//! Common step definitions shared across all features

use cucumber::{given, then};
use crate::world::AlephWorld;
use tempfile::tempdir;

#[given("a temporary directory")]
async fn given_temp_dir(w: &mut AlephWorld) {
    w.temp_dir = Some(tempdir().expect("Failed to create temp dir"));
}

#[then("the operation should succeed")]
async fn then_should_succeed(w: &mut AlephWorld) {
    match &w.last_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Expected success, got error: {}", e),
        None => panic!("No operation result recorded"),
    }
}

#[then("the operation should fail")]
async fn then_should_fail(w: &mut AlephWorld) {
    match &w.last_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected failure, but operation succeeded"),
        None => panic!("No operation result recorded"),
    }
}

#[then(expr = "the error message should contain {string}")]
async fn then_error_contains(w: &mut AlephWorld, expected: String) {
    let err = w.last_error.as_ref().expect("No error recorded");
    assert!(
        err.contains(&expected),
        "Error '{}' does not contain '{}'",
        err,
        expected
    );
}
