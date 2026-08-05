//! Single predicate for session ownership visibility (P1 data isolation).
//!
//! `SessionMetadata::owner_user_id` is `None` on legacy/pre-P1 rows and on
//! rows created outside any dispatch scope (cron, internal, A2A). Those rows
//! read as owned by the org-era single operator — adoption-by-absence, not a
//! missing value. [`effective_owner`] is the one place that rule is encoded;
//! both `SessionStore` backends' `SessionFilter::owner_visible_to` filter
//! call it, so the fallback can never drift between them.

use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::session_store::types::SessionMetadata;

/// The user who effectively owns `meta` for visibility purposes: its stamped
/// `owner_user_id`, or [`OWNER_USER_ID`] for a legacy/pre-P1 row with no
/// scope stamp.
#[must_use]
pub fn effective_owner(meta: &SessionMetadata) -> &str {
    meta.owner_user_id.as_deref().unwrap_or(OWNER_USER_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamped_row_reads_its_own_owner() {
        let meta = SessionMetadata {
            owner_user_id: Some("u-alice".to_string()),
            ..Default::default()
        };
        assert_eq!(effective_owner(&meta), "u-alice");
    }

    #[test]
    fn legacy_row_reads_as_owner_by_absence() {
        let meta = SessionMetadata::default();
        assert_eq!(effective_owner(&meta), OWNER_USER_ID);
    }
}
