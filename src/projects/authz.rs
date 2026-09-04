//! Who may see and who may reconfigure a project room — one derivation, two
//! faces.
//!
//! The `projects.*` RPC handlers had these rules inline, reading the ambient
//! task-local. That is correct for an RPC (the gateway sets `CALLER_USER` per
//! request) and structurally wrong for a tool: inside a spawned run the
//! task-local is `None`, and `None` is the UNRESTRICTED arm every predicate in
//! this codebase opens with. A tool face that reached for the ambient form
//! would therefore be admitted unconditionally — not refused, admitted — with
//! nothing anywhere saying so.
//!
//! So the rules take the actor explicitly and both faces pass in whatever
//! their own surface knows: the RPC passes `visibility::visible_owner_filter()`,
//! the tool passes `visibility::ambient_actor()`. Neither owns the rule.
//!
//! ## There is no `Forbidden`
//!
//! An earlier sketch of this module had `ProjectAccess::{Ok, NotFound,
//! Forbidden}`. Nothing can produce `Forbidden`: a room the actor is not on
//! the roster of must be indistinguishable from a room that does not exist,
//! or the refusal itself tells a stranger the room is real. A store error
//! collapses to the same answer, because a caller must never be admitted
//! because the check could not complete. That leaves exactly two outcomes, so
//! the type is `Option<Project>` and the variant nobody could produce is not
//! here to be reached for later.

use super::{Project, ProjectStore};

/// The project `actor` may address by `id`, or `None`.
///
/// `None` covers all three of "no such project", "not on its roster" and "the
/// store could not answer" — deliberately one answer, see the module doc.
/// `actor: None` is the unrestricted caller (cron, A2A, in-process), matching
/// every other predicate.
#[must_use]
pub fn project_for(store: &ProjectStore, id: &str, actor: Option<&str>) -> Option<Project> {
    match store.get(id) {
        Ok(Some(p)) if crate::gateway::visibility::project_visible_to(&p.id, actor) => Some(p),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(project_id = %id, error = %e, "projects: authz gate failed closed");
            None
        }
    }
}

/// Whether `actor` may reconfigure `project` (rename, archive, roster, bind).
///
/// Assumes visibility already passed — this answers "is this room yours to
/// reconfigure", not "may you see it". Calling it alone would answer a
/// question about a room the actor cannot address.
///
/// Org admins pass for any room (spec §6.3: owner changes are an admin
/// operation); resolving *whether* the actor is an admin needs the security
/// store, which stays with the caller so this module holds no store of its
/// own — [`is_active_principal`] below takes one by parameter for the same
/// reason.
#[must_use]
pub fn is_owner(project: &Project, actor: Option<&str>, actor_is_admin: bool) -> bool {
    let Some(caller) = actor else {
        return true;
    };
    caller == crate::gateway::visibility::owner_or_legacy(project.owner_user_id.as_deref())
        || actor_is_admin
}

/// The refusal both faces give for removing a room's owner, verbatim.
///
/// The text is shared rather than re-typed because the rule is one rule: the
/// RPC handler and `project_manage`'s `member_remove` arm are two surfaces on
/// it, and two spellings become two rules the first time one is edited
/// (criterion #1).
pub const OWNER_REMOVAL_REFUSAL: &str = "cannot remove the project owner from its own roster";

/// Whether `target` may be dropped from `project`'s roster.
///
/// The owner is the one member who cannot be removed: the roster IS the
/// visibility predicate, so dropping them leaves the room addressable by
/// nobody — not even the org admin who could archive it. Hand the room over
/// first (an admin operation), then remove.
///
/// **Target-based, deliberately not caller-based.** [`is_owner`] answers a
/// question about the CALLER and returns `true` for `actor: None`, the
/// unattributed run every spawned tool call is. A caller-shaped spelling of
/// this rule would therefore leave that arm open on the one mutation that can
/// make a room invisible to everyone.
#[must_use]
pub fn may_remove_member(project: &Project, target: &str) -> bool {
    target != crate::gateway::visibility::owner_or_legacy(project.owner_user_id.as_deref())
}

/// The refusal both faces give for a roster mutation naming somebody who is
/// not an active principal. Shared for the same reason as
/// [`OWNER_REMOVAL_REFUSAL`].
#[must_use]
pub fn unknown_user_refusal(user_id: &str) -> String {
    format!("unknown user: {user_id}")
}

/// Whether `user_id` names a principal a roster may currently name.
///
/// A deactivated user reads as unknown on purpose: seating one grants access
/// that materialises if they are ever reactivated, which is a decision
/// `users.update` owns, not a roster verb. Fails closed on a store error — an
/// id that could not be verified is not a verified id (criterion #8).
///
/// The store arrives by parameter: this module holds none, so the rule can be
/// asked by a face that has one and stays unasked by a face that does not.
#[must_use]
pub fn is_active_principal(
    users: &crate::gateway::security::store::SecurityStore,
    user_id: &str,
) -> bool {
    use crate::gateway::security::store::UserStatus;
    match users.get_user(user_id) {
        Ok(Some(u)) => u.status == UserStatus::Active,
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "projects: principal check failed closed"
            );
            false
        }
    }
}

/// The refusal a name that fits more than one active principal gives.
///
/// It NAMES every candidate, because the caller's only way forward is to
/// re-ask by id, and it cannot do that from a refusal that says only "that
/// was ambiguous". Status rides along so the answer reads the same as
/// `users.list` would: the same three columns, in the server's own spelling.
#[must_use]
pub fn ambiguous_user_refusal(
    name: &str,
    candidates: &[aleph_protocol::users::UserView],
) -> String {
    let listed = candidates
        .iter()
        .map(|c| format!("{} ({}, {})", c.user_id, c.display_name, c.status))
        .collect::<Vec<_>>()
        .join("; ");
    format!("'{name}' names more than one user; re-ask with one of these ids: {listed}")
}

/// Resolve a display name to the one active principal that bears it, or the
/// refusal to relay.
///
/// This is the whole name→principal path, and it is the ONLY one: a caller
/// that wants an id from a name asks here, so the fail-closed rules below are
/// asked once rather than re-derived per face.
///
/// **Every unresolved path answers `Err`, and none of them may be read as a
/// value** (criterion #8). A store that cannot be listed, a name nobody
/// bears, and a name only a deactivated principal bears all produce
/// [`unknown_user_refusal`] — the same string an unknown *id* produces on the
/// same verb. Three reasons, one answer, deliberately: a distinct refusal for
/// the deactivated case would turn this into an existence oracle over the
/// principal directory, and passing the name through as an id would turn "I
/// do not know" into a `project_members` row nobody owns.
///
/// The status narrowing inside `aleph_protocol::users::resolve` is not a
/// second spelling of [`is_active_principal`]: it filters a projection, and
/// the winner is then put to that one predicate — which reads the store —
/// before this returns an id. A view that has gone stale therefore cannot
/// admit anybody.
pub fn principal_id_for_name(
    users: &crate::gateway::security::store::SecurityStore,
    name: &str,
) -> std::result::Result<String, String> {
    use aleph_protocol::users::Resolution;

    let records = users.list_users().map_err(|e| {
        tracing::warn!(error = %e, "projects: name resolution failed closed");
        unknown_user_refusal(name)
    })?;
    let views: Vec<aleph_protocol::users::UserView> = records
        .into_iter()
        .map(crate::gateway::handlers::users::user_view)
        .collect();

    match aleph_protocol::users::resolve(name, &views) {
        Resolution::One(view) if is_active_principal(users, &view.user_id) => Ok(view.user_id),
        // The projection said active and the store disagreed. That is the
        // stale-view case, and the store wins.
        Resolution::One(_) | Resolution::None => Err(unknown_user_refusal(name)),
        Resolution::Ambiguous(candidates) => Err(ambiguous_user_refusal(name, &candidates)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of three: two live principals and one switched-off one.
    fn directory() -> crate::gateway::security::store::SecurityStore {
        use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
        let users = SecurityStore::in_memory().unwrap();
        users.create_user("u-ada", "Ada", UserRole::Member).unwrap();
        users.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        users
            .create_user("u-gone", "Ghost", UserRole::Member)
            .unwrap();
        users
            .update_user("u-gone", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        users
    }

    #[test]
    fn a_name_borne_by_one_active_principal_resolves_to_their_id() {
        let users = directory();
        assert_eq!(principal_id_for_name(&users, "Ada").as_deref(), Ok("u-ada"));
        assert_eq!(
            principal_id_for_name(&users, " bob ").as_deref(),
            Ok("u-bob"),
            "a relayed name arrives with the human's spacing and casing"
        );
    }

    /// The three unresolved reasons answer with ONE string — the same one an
    /// unknown *id* gets — so the refusal is not an existence oracle over the
    /// principal directory, and so no caller can branch on the difference.
    #[test]
    fn every_unresolved_reason_gives_the_same_refusal_an_unknown_id_gives() {
        let users = directory();
        assert_eq!(
            principal_id_for_name(&users, "Ghost"),
            Err(unknown_user_refusal("Ghost")),
            "a deactivated bearer must read exactly like nobody"
        );
        assert_eq!(
            principal_id_for_name(&users, "Nobody"),
            Err(unknown_user_refusal("Nobody"))
        );
        assert_eq!(
            principal_id_for_name(&users, ""),
            Err(unknown_user_refusal(""))
        );
    }

    /// The `Err` arm of `list_users` (criterion #8): a store that cannot
    /// answer must refuse, not resolve. Without this the fail-closed comment
    /// on that arm is unverified prose.
    #[test]
    fn a_store_that_cannot_be_listed_resolves_nobody() {
        let users = directory();
        users
            .conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DROP TABLE users", [])
            .unwrap();
        assert_eq!(
            principal_id_for_name(&users, "Ada"),
            Err(unknown_user_refusal("Ada")),
            "a store error must not resolve to the name itself, nor to anybody"
        );
    }

    /// Ambiguity names every candidate, because "that was ambiguous" alone
    /// leaves the caller with no way to re-ask.
    #[test]
    fn an_ambiguous_name_refuses_and_names_every_candidate() {
        use crate::gateway::security::store::UserRole;
        let users = directory();
        users
            .create_user("u-ada2", "ada", UserRole::Member)
            .unwrap();

        let refusal = principal_id_for_name(&users, "Ada").expect_err("two bearers, no winner");
        assert!(refusal.contains("u-ada"), "got: {refusal}");
        assert!(refusal.contains("u-ada2"), "got: {refusal}");
        assert!(
            !refusal.contains(&unknown_user_refusal("Ada")),
            "ambiguity is not absence: {refusal}"
        );
    }

    /// The projection and the store are two readings of one fact, and when
    /// they disagree the STORE wins. `scope::directory`'s cache is allowed to
    /// be stale precisely because nothing authorization-shaped reads it; this
    /// asserts the resolver honours that by re-asking `is_active_principal`
    /// rather than trusting the view it just filtered.
    #[test]
    fn the_store_and_not_the_projection_admits_the_winner() {
        use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
        let users = SecurityStore::in_memory().unwrap();
        users.create_user("u-ada", "Ada", UserRole::Member).unwrap();
        assert_eq!(principal_id_for_name(&users, "Ada").as_deref(), Ok("u-ada"));

        users
            .update_user("u-ada", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        assert_eq!(
            principal_id_for_name(&users, "Ada"),
            Err(unknown_user_refusal("Ada")),
            "the one active-principal predicate decides, on every path"
        );
    }

    /// The cross-crate half of "active has one spelling": a record the server
    /// calls Active must satisfy the protocol crate's `is_active`, or the
    /// resolver's status filter silently matches nobody (criterion #10).
    #[test]
    fn the_servers_active_and_the_contracts_active_are_the_same_string() {
        let users = directory();
        let record = users.get_user("u-ada").unwrap().expect("Ada exists");
        assert!(crate::gateway::handlers::users::user_view(record).is_active());
    }
    use crate::projects::roster::TEST_GUARD;
    use rusqlite::Connection;

    fn store_with_room() -> (ProjectStore, Project, std::sync::MutexGuard<'static, ()>) {
        let guard = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let project = store.create("room", Some("u-alice"), None).unwrap();
        store.add_member(&project.id, "u-bob").unwrap();
        (store, project, guard)
    }

    #[test]
    fn a_member_addresses_the_room_and_a_stranger_cannot_tell_it_from_absent() {
        let (store, project, _g) = store_with_room();
        assert!(project_for(&store, &project.id, Some("u-bob")).is_some());

        let refused = project_for(&store, &project.id, Some("u-mallory"));
        let absent = project_for(&store, "p-nope", Some("u-mallory"));
        assert!(refused.is_none());
        assert!(absent.is_none(), "refusal and absence are one answer");
    }

    #[test]
    fn an_unrestricted_caller_addresses_any_existing_room_but_not_a_missing_one() {
        let (store, project, _g) = store_with_room();
        assert!(project_for(&store, &project.id, None).is_some());
        assert!(
            project_for(&store, "p-nope", None).is_none(),
            "unrestricted is not omniscient: a missing room is still missing"
        );
    }

    #[test]
    fn only_the_owner_or_an_admin_may_reconfigure() {
        let (_store, project, _g) = store_with_room();
        assert!(is_owner(&project, Some("u-alice"), false), "the owner");
        assert!(is_owner(&project, Some("u-carol"), true), "an org admin");
        assert!(
            !is_owner(&project, Some("u-bob"), false),
            "a plain member may not reconfigure"
        );
        assert!(is_owner(&project, None, false), "unrestricted passes");
    }

    #[test]
    fn the_owner_is_the_one_member_who_cannot_be_dropped() {
        let (_store, project, _g) = store_with_room();
        assert!(!may_remove_member(&project, "u-alice"), "the owner stays");
        assert!(may_remove_member(&project, "u-bob"), "a plain member goes");

        let legacy = Project {
            owner_user_id: None,
            ..project
        };
        assert!(
            !may_remove_member(&legacy, crate::gateway::security::store::OWNER_USER_ID),
            "a pre-P1 row's owner is the legacy owner, not nobody"
        );
    }

    #[test]
    fn a_deactivated_or_absent_principal_is_not_one_a_roster_may_name() {
        use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
        let users = SecurityStore::in_memory().unwrap();
        users
            .create_user("u-live", "u-live", UserRole::Member)
            .unwrap();
        users
            .create_user("u-gone", "u-gone", UserRole::Member)
            .unwrap();
        users
            .update_user("u-gone", None, None, Some(UserStatus::Deactivated))
            .unwrap();

        assert!(is_active_principal(&users, "u-live"));
        assert!(
            !is_active_principal(&users, "u-gone"),
            "deactivated reads as unknown"
        );
        assert!(
            !is_active_principal(&users, "u-never"),
            "absent reads as unknown"
        );
        assert!(!is_active_principal(&users, ""), "the empty id is nobody");
    }

    /// The `Err` arm specifically (criterion #8): none of the cases above
    /// reach it — an absent or deactivated row is `Ok`, not `Err`. Drop the
    /// backing table so `get_user` genuinely errors, and confirm the
    /// predicate still refuses rather than reading the failure as
    /// permission. Without this test the fail-closed comment on the `Err`
    /// arm is unverified prose: flipping that arm to `true` passes every
    /// other test in this module untouched.
    #[test]
    fn a_store_error_is_not_read_as_permission() {
        use crate::gateway::security::store::SecurityStore;
        let users = SecurityStore::in_memory().unwrap();
        users
            .conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DROP TABLE users", [])
            .unwrap();

        assert!(
            !is_active_principal(&users, "u-anyone"),
            "a store error must fail closed, not open"
        );
    }
}
