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
/// store, which stays with the caller so this module depends on nothing but
/// the project row.
#[must_use]
pub fn is_owner(project: &Project, actor: Option<&str>, actor_is_admin: bool) -> bool {
    let Some(caller) = actor else {
        return true;
    };
    caller == crate::gateway::visibility::owner_or_legacy(project.owner_user_id.as_deref())
        || actor_is_admin
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
