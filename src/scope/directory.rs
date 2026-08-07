//! In-process projection of `users.display_name`.
//!
//! The SSOT is the `users` table (`gateway::security::store::users`); this is a
//! read-optimised snapshot, for the same reason [`crate::projects::roster`]
//! exists: the consumer is a **synchronous** function on a hot path. Here the
//! consumer is `thinker::nudges::speaker_label`, called once per user message
//! while rebuilding the prompt — a SQLite round-trip per message per turn is
//! not an option, and making the prompt builder async would spread virally into
//! `src/harness/` (R10).
//!
//! ## Why this is a cache and the roster is a projection
//!
//! [`crate::projects::roster`] REPLACES its whole snapshot on every write,
//! because it answers an **authorization** question (`is_member`) where a stale
//! `true` is a security bug. This map answers a **presentation** question, so
//! it upserts instead:
//!
//! - a missing entry degrades to rendering the raw user id — correct, just ugly;
//! - a stale entry renders the name someone used to have — wrong-looking, never
//!   unsafe;
//! - a deleted user leaves a name behind, which nothing reads (their id stops
//!   appearing on new messages).
//!
//! Upsert also means two `cargo test` threads with their own in-memory stores do
//! not erase each other, so unlike the roster this needs no test guard.
//!
//! Cross-process caveat: a second process writing `security.db` is not seen
//! here. Same standing rule as the roster — user mutation is RPC-only.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

fn cell() -> &'static RwLock<HashMap<String, String>> {
    static NAMES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    NAMES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Upsert one user's display name. Called by `SecurityStore` after every write
/// that can change it (`create_user`, `update_user`).
pub fn record(user_id: &str, display_name: &str) {
    let mut guard = cell().write().unwrap_or_else(|e| e.into_inner());
    guard.insert(user_id.to_string(), display_name.to_string());
}

/// Seed the map from the `users` table. Called once at boot, because the
/// per-write hook above only sees writes made by THIS process — a server that
/// restarts would otherwise render bare ids until someone happened to be
/// renamed.
pub fn hydrate(pairs: impl IntoIterator<Item = (String, String)>) {
    let mut guard = cell().write().unwrap_or_else(|e| e.into_inner());
    guard.extend(pairs);
}

/// The display name recorded for `user_id`, or `None` when this process has
/// never seen it.
///
/// `None` is not an error and must not be treated as "no such user" — it means
/// "no nicer name available", and every caller is expected to fall back to the
/// id itself.
#[must_use]
pub fn display_name(user_id: &str) -> Option<String> {
    cell()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(user_id)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_user_has_no_name_rather_than_an_empty_one() {
        // The caller distinguishes "no nicer name" from "named the empty
        // string"; collapsing them would render `[]: ...`.
        assert_eq!(display_name("u-nobody-has-ever-seen-this"), None);
    }

    #[test]
    fn a_rename_overwrites_rather_than_accumulates() {
        record("u-directory-rename", "Ada");
        assert_eq!(display_name("u-directory-rename").as_deref(), Some("Ada"));
        record("u-directory-rename", "Ada Lovelace");
        assert_eq!(
            display_name("u-directory-rename").as_deref(),
            Some("Ada Lovelace")
        );
    }

    #[test]
    fn hydrate_merges_and_does_not_erase_entries_it_did_not_carry() {
        // This is the difference from `roster::publish`, and it is the whole
        // reason this map needs no test guard: a second store hydrating its own
        // users must not blank out the first one's.
        record("u-directory-survivor", "Grace");
        hydrate([("u-directory-newcomer".to_string(), "Alan".to_string())]);
        assert_eq!(
            display_name("u-directory-survivor").as_deref(),
            Some("Grace")
        );
        assert_eq!(
            display_name("u-directory-newcomer").as_deref(),
            Some("Alan")
        );
    }
}
