//! ARIA snapshot module — pending migration in Task 10.
//!
//! The original chromiumoxide-based ARIA snapshot implementation has been
//! stubbed out as part of the text-first BrowserBackend refactor. The
//! playwright-cli backend will provide snapshots natively; this module
//! will be deleted in Task 10.

use chromiumoxide::Page;

use super::error::BrowserError;

/// Take an ARIA snapshot — pending migration, always returns error.
pub async fn take_aria_snapshot(_page: &Page) -> Result<(), BrowserError> {
    Err(BrowserError::ActionFailed("ARIA snapshot pending migration".into()))
}

/// Resolve a ref_id to a point — pending migration, always returns None.
pub fn resolve_ref_to_point(_snapshot: &(), _ref_id: &str) -> Option<(f64, f64)> {
    None
}
