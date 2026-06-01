//! Session → active-scratchpad pointer registry.
//!
//! The `scratchpad` tool is keyed by an LLM-chosen `project_id`, *not* by
//! session. To let the goal-loop hook ([`crate::verification::ScratchpadGoalVerifier`])
//! find *which* execution list belongs to the session that is about to
//! stop, the tool records its most-recently-touched `project_id` here,
//! keyed by the live session key. The verifier reads it back at stop time.
//!
//! This is a pure runtime pointer with no persistence: the scratchpad
//! markdown file on disk remains the single source of truth for the
//! execution-list contents. Mirrors the process-global table pattern of
//! [`crate::builtin_tools::process_registry`].
//!
//! R10 note: this is plumbing, not cognition — it answers the purely
//! mechanical question "which project file is this session writing to?".
//! It performs no judgment of its own.

use std::collections::HashMap;

use once_cell::sync::Lazy;

use crate::sync_primitives::{Mutex, MutexGuard};

static ACTIVE: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn lock() -> MutexGuard<'static, HashMap<String, String>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record `project_id` as the active execution list for `session_key`.
///
/// No-op when either key is empty (an unbound session must not shadow a
/// real one under the empty-string key).
pub fn set_active(session_key: &str, project_id: &str) {
    if session_key.is_empty() || project_id.is_empty() {
        return;
    }
    lock().insert(session_key.to_string(), project_id.to_string());
}

/// The active execution-list `project_id` for `session_key`, if any.
pub fn active(session_key: &str) -> Option<String> {
    if session_key.is_empty() {
        return None;
    }
    lock().get(session_key).cloned()
}

/// Drop the pointer for `session_key` (e.g. the scratchpad was cleared).
pub fn clear(session_key: &str) {
    lock().remove(session_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_active_then_read_roundtrips() {
        set_active("sess-roundtrip", "proj-a");
        assert_eq!(active("sess-roundtrip"), Some("proj-a".to_string()));
        clear("sess-roundtrip");
        assert_eq!(active("sess-roundtrip"), None);
    }

    #[test]
    fn empty_keys_are_ignored() {
        set_active("", "proj-x");
        assert_eq!(active(""), None);
        set_active("sess-empty-proj", "");
        assert_eq!(active("sess-empty-proj"), None);
    }

    #[test]
    fn latest_write_wins() {
        set_active("sess-latest", "proj-1");
        set_active("sess-latest", "proj-2");
        assert_eq!(active("sess-latest"), Some("proj-2".to_string()));
        clear("sess-latest");
    }
}
