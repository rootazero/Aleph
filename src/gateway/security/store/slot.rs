//! The process-global handle to the users table, for code that runs with
//! no request in hand.
//!
//! Two consumers today: the fire-time authority resolver
//! (`crate::scope::authority::resolve`), which every background trigger asks
//! "is the person this work acts for still here, and in what role?", and
//! [`multi_user`], the one answer to "does this server have people besides
//! the machine owner?". A trigger has no gateway dispatch around it, so it
//! cannot be handed the store the way an RPC handler is; threading it through
//! each executor's constructor is the per-executor parameter chain this slot
//! replaces.

use super::{SecurityStore, OWNER_USER_ID};
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// What boot installed: the store, and whether it is the in-memory fallback
/// `initialize_vault` opens when the on-disk `security.db` cannot be opened.
pub struct InstalledUsersStore {
    store: Arc<SecurityStore>,
    /// `Some(reason)` when the store is the in-memory fallback. Its users
    /// table holds only the bootstrap owner, so "no such row" there says
    /// nothing about whether a person exists.
    degraded: Option<String>,
}

/// `FailsOpen`, named as what it is: with nothing installed the resolver
/// answers `Legacy` for every subject, so a deactivated principal's cron
/// job fires and a demoted admin's heartbeat runs as operator — the
/// fire-time gate silently stops gating. That is also exactly the
/// behaviour tests and minimal servers have always had (they never had a
/// store to ask), which is why the variant is honest rather than alarming
/// there, and why boot installs this unconditionally: `initialize_vault`
/// always yields a store (on disk, else in memory — the latter installed
/// DEGRADED, see [`install_degraded_users_store`]), so there is no decline
/// arm to write.
static USERS_STORE: CapabilitySlot<InstalledUsersStore> =
    CapabilitySlot::new("security/users-store", MissingSemantics::FailsOpen);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn users_store_slot() -> &'static dyn SlotStatus {
    &USERS_STORE
}

/// Install the process-wide users store. Idempotent — a second call is
/// ignored (mirrors `spend::install_ledger`). Boot calls this once, right
/// after `initialize_vault`, with the SAME `Arc` every other durable
/// consumer shares, so a deactivation or demotion `users.update` writes is
/// what the next background fire reads.
pub fn install_users_store(store: Arc<SecurityStore>) {
    let _ = USERS_STORE.install(InstalledUsersStore {
        store,
        degraded: None,
    });
}

/// Install the in-memory FALLBACK store, marked degraded with `reason`.
///
/// RPC handlers never read this slot (they are handed the store), so they
/// keep using the fallback exactly as before. Only the two readers of
/// [`users_authority`] see the mark: the fire-time resolver answers
/// `Unknown` (not `Gone`) for anyone but the owner, and [`multi_user`] answers
/// `true` — a table that lost its rows is no evidence of a single-user
/// install (判据 §8, §15).
pub fn install_degraded_users_store(store: Arc<SecurityStore>, reason: String) {
    let _ = USERS_STORE.install(InstalledUsersStore {
        store,
        degraded: Some(reason),
    });
}

/// What the users table can vouch for — the slot's state as the fire-time
/// resolver and [`multi_user`] read it, and the seam their tests inject
/// (no lib test installs the process global).
#[derive(Clone, Copy)]
pub enum UsersAuthority<'a> {
    /// Nothing installed: tests and minimal servers.
    Absent,
    /// The durable users table.
    Durable(&'a SecurityStore),
    /// The in-memory fallback: it holds only the bootstrap owner.
    Degraded {
        store: &'a SecurityStore,
        reason: &'a str,
    },
}

impl<'a> UsersAuthority<'a> {
    /// A store handed in explicitly is taken as durable; `None` is absent.
    #[must_use]
    pub fn from_store(store: Option<&'a SecurityStore>) -> Self {
        store.map_or(Self::Absent, Self::Durable)
    }
}

/// The installed slot as a [`UsersAuthority`].
#[must_use]
pub(crate) fn users_authority() -> UsersAuthority<'static> {
    match USERS_STORE.get() {
        None => UsersAuthority::Absent,
        Some(InstalledUsersStore {
            store,
            degraded: None,
        }) => UsersAuthority::Durable(store),
        Some(InstalledUsersStore {
            store,
            degraded: Some(reason),
        }) => UsersAuthority::Degraded { store, reason },
    }
}

/// Does this server have people besides the machine owner? — THE one
/// derivation of "multi-user mode", read from table content, never from
/// whether a store is installed (boot installs one on every server, so that
/// test is constant-true in production, 判据 §2).
///
/// Consumers — the None-principal ruling's two call sites,
/// `visibility::run_principal` (room creation, legacy `memory_events` rows)
/// and `browser_tools::caller_browser_principal` (profile selection), and the
/// room floor of the three session-row executors, which hand this function
/// to `fire_gate::authorize_session_run` so it is read only when the floor is
/// in question (`admit_reinjection`, `admit_announce`, `admit_resume`).
/// `multi_user_derivation_census` pins that list.
/// `scope::authority`'s `FireAuthority::Legacy` does NOT route through this:
/// it answers a different question (is there a store to check a person
/// against at all).
#[must_use]
pub(crate) fn multi_user() -> bool {
    multi_user_in(users_authority())
}

/// [`multi_user`] for an explicit store — `None` is "no store installed".
/// The test seam: no lib test installs the process global.
#[cfg(test)]
#[must_use]
pub(crate) fn multi_user_with(store: Option<&SecurityStore>) -> bool {
    multi_user_in(UsersAuthority::from_store(store))
}

/// | slot state | answer |
/// |---|---|
/// | nothing installed | `false` |
/// | only `OWNER_USER_ID` has a row | `false` |
/// | any other row, in any status | `true` |
/// | the read fails | `true` (fail closed, 判据 §8) |
/// | the in-memory fallback | `true` (its table lost the rows that would say) |
#[must_use]
pub(crate) fn multi_user_in(users: UsersAuthority<'_>) -> bool {
    match users {
        UsersAuthority::Absent => false,
        UsersAuthority::Degraded { .. } => true,
        UsersAuthority::Durable(store) => store.has_user_other_than(OWNER_USER_ID).unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id. `FailsOpen` is the variant that makes `aleph doctor` exit
    /// non-zero when the slot is missing; changing it means re-reading the
    /// resolver, not re-typing this line.
    #[test]
    fn the_users_store_slot_pins_its_missing_semantics() {
        assert_eq!(users_store_slot().id(), "security/users-store");
        assert!(
            matches!(users_store_slot().missing(), MissingSemantics::FailsOpen),
            "`security/users-store` is FailsOpen: an uninstalled store makes \
             every fire-time authority check answer Legacy"
        );
    }

    /// "Multi-user" is table content: owner-only is single-user, any other
    /// row (active or not) is multi-user, and the fallback store is treated
    /// as multi-user because it lost the rows that would say otherwise.
    #[test]
    fn multi_user_mode_is_read_from_the_users_table() {
        use crate::gateway::security::store::{UserRole, UserStatus};
        assert!(!multi_user_with(None), "no store installed");

        let store = SecurityStore::in_memory().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        assert!(!multi_user_with(Some(&store)), "only the owner has a row");

        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        assert!(
            multi_user_with(Some(&store)),
            "a second person, even a deactivated one"
        );

        let fallback = SecurityStore::in_memory().unwrap();
        fallback.ensure_bootstrap_owner().unwrap();
        assert!(multi_user_in(UsersAuthority::Degraded {
            store: &fallback,
            reason: "security.db could not be opened",
        }));

        let broken = SecurityStore::in_memory().unwrap();
        broken
            .conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute_batch("DROP TABLE users")
            .unwrap();
        assert!(
            multi_user_with(Some(&broken)),
            "a users read that fails is not evidence of a single-user install"
        );
    }

    /// Re-review N2: every consumer of [`multi_user`] is tested through an
    /// explicit-mode seam (`visibility::run_principal_in`,
    /// `browser_tools::caller_browser_principal_in`, the `multi_user`
    /// argument of `fire_gate::authorize_session_run`), so a consumer that
    /// stopped reading the real mode — `false`, or the constant-true "a store
    /// is installed" this function replaced — keeps every behavioural test
    /// green. This pins the wire at the source, over production code (tests,
    /// comments and literals stripped):
    /// - `run_principal()` and `caller_browser_principal()` each call
    ///   `slot::multi_user()` in their own body;
    /// - every production reference to `slot::multi_user` (a call or the
    ///   function handed on as the floor's mode), per file, is exactly the
    ///   table below — the two faces plus the three session-row executors;
    /// - `multi_user_in(` has one production caller: [`multi_user`] itself.
    ///
    /// Counted by the path spelling: a file that imports `multi_user` and
    /// calls it bare is seen once, at its import.
    #[test]
    fn multi_user_derivation_census() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};
        const EXPECTED: [(&str, usize); 5] = [
            ("src/builtin_tools/browser_tools/mod.rs", 1),
            ("src/gateway/announce_delivery.rs", 1),
            ("src/gateway/busy_queue/durable.rs", 1),
            ("src/gateway/resume_coordinator.rs", 1),
            ("src/gateway/visibility.rs", 1),
        ];
        let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let whole = |code: &str, token: &str| -> usize {
            code.match_indices(token)
                .filter(|(at, _)| {
                    let before = code.get(..*at).unwrap_or_default();
                    let after = code.get(*at + token.len()..).unwrap_or_default();
                    !before.chars().next_back().is_some_and(is_ident)
                        && !after.chars().next().is_some_and(is_ident)
                        && !before.trim_end().ends_with("fn")
                })
                .count()
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let code: std::collections::BTreeMap<String, String> = rust_sources_under(&root)
            .into_iter()
            .map(|(rel, text)| {
                let production = code_text(&production_text(std::path::Path::new(&rel), &text));
                (rel, production)
            })
            .collect();

        for (file, function) in [
            ("src/gateway/visibility.rs", "fn run_principal()"),
            (
                "src/builtin_tools/browser_tools/mod.rs",
                "fn caller_browser_principal()",
            ),
        ] {
            let text = code.get(file).unwrap_or_else(|| panic!("{file} is gone"));
            let start = text
                .find(function)
                .unwrap_or_else(|| panic!("the scan is broken, not the tree: no `{function}`"));
            let body = text
                .get(start..)
                .and_then(|rest| rest.find("\n}").and_then(|end| rest.get(..end)))
                .unwrap_or_else(|| panic!("`{function}`'s body never closes at column 0"));
            assert!(
                body.contains("slot::multi_user()"),
                "{file}: `{function}` must read the server's mode from `slot::multi_user()` — \
                 its tests run through an explicit-mode seam and cannot see this wire"
            );
        }

        let found: std::collections::BTreeMap<&str, usize> = code
            .iter()
            .map(|(rel, text)| (rel.as_str(), whole(text.as_str(), "slot::multi_user")))
            .filter(|(_, n)| *n > 0)
            .collect();
        let expected: std::collections::BTreeMap<&str, usize> = EXPECTED.into_iter().collect();
        assert_eq!(
            found, expected,
            "a production reference to `slot::multi_user` appeared, moved or went away — a \
             consumer that hands its floor or its principal verdict a constant instead of the \
             server's mode is green in every behavioural test"
        );

        let callers: std::collections::BTreeMap<&str, usize> = code
            .iter()
            .map(|(rel, text)| (rel.as_str(), whole(text.as_str(), "multi_user_in")))
            .filter(|(_, n)| *n > 0)
            .collect();
        let only_multi_user: std::collections::BTreeMap<&str, usize> =
            [("src/gateway/security/store/slot.rs", 1)]
                .into_iter()
                .collect();
        assert_eq!(
            callers, only_multi_user,
            "`multi_user_in` has one production caller, `multi_user`; a second derivation of \
             the mode is the one this function exists to prevent (判据 §1)"
        );

        // The token rule itself: a call and a pointer count; the definition,
        // a longer name and the `_in` / `_with` seams do not (判据 §3).
        assert_eq!(
            whole(
                "pub(crate) fn multi_user() {}\nlet a = slot::multi_user();\n\
                 f(slot::multi_user, row);\nlet b = slot::multi_user_in(x);\n\
                 let c = slot::multi_user_with(None);",
                "slot::multi_user"
            ),
            2
        );
    }

    /// No lib test may install this process-global. Once installed it
    /// stays for the whole test binary, and every other test that fires an
    /// owned cron job or heartbeat would then resolve its owner against
    /// that store — and be refused as `Gone`. Tests inject a store through
    /// `scope::authority::resolve_with` instead.
    #[test]
    fn no_lib_test_installs_the_process_global_users_store() {
        assert!(
            matches!(users_authority(), UsersAuthority::Absent),
            "some lib test called install_users_store; use resolve_with"
        );
    }
}
