//! Canonical spelling of a scope id — the string form that crosses the wire.
//!
//! `alephcore`'s `scope::ScopeId` is the typed owner of this vocabulary, but
//! it lives behind a crate boundary the Panel cannot cross (`interfaces/
//! webchat` deliberately depends on `aleph-protocol`, never on `alephcore`).
//! Without a shared home, every client that wants to say "this row belongs to
//! project p" spells `format!("project:{id}")` by hand — a second answer to a
//! question `ScopeId::render` already answers, and the kind that drifts
//! silently: a client filtering on a prefix the server no longer emits gets an
//! empty list, which renders exactly like "this room has nothing in it".
//!
//! So the *spelling* lives here and `ScopeId::render`/`parse` are written in
//! terms of it (see that type's reconciliation test, which fails if the two
//! ever disagree). This module is intentionally strings-only: the typed enum,
//! its `Org` arm, and every authorization predicate stay in core, because a
//! client has no business deciding what a scope *permits* — only what it is
//! *called*.

/// Organization-wide scope, which is rendered whole rather than prefixed.
pub const ORG: &str = "org";

/// Prefix of a personal scope id: `personal:<user_id>`.
pub const PERSONAL_PREFIX: &str = "personal:";

/// Prefix of a project (room) scope id: `project:<project_id>`.
pub const PROJECT_PREFIX: &str = "project:";

/// Render the scope id for `project_id`.
///
/// Takes the bare project id (`p-x7f2`), never an already-rendered scope id —
/// double-prefixing produces `project:project:p-x7f2`, which parses to
/// `None` and fails closed to an empty list rather than to an error, so the
/// mistake is invisible at the call site. [`project_id_of`] is the inverse.
#[must_use]
pub fn project_scope_id(project_id: &str) -> String {
    format!("{PROJECT_PREFIX}{project_id}")
}

/// The project id inside a project scope id, or `None` for any other scope.
///
/// An empty ref (`"project:"`) is `None`, matching core's parser: a prefix
/// with nothing after it names no project, and admitting it would let an
/// empty filter match rows stamped for a real room.
#[must_use]
pub fn project_id_of(scope_id: &str) -> Option<&str> {
    scope_id
        .strip_prefix(PROJECT_PREFIX)
        .filter(|rest| !rest.is_empty())
}

/// Whether a row stamped `scope_id` belongs to `project_id`.
///
/// `None` — an unstamped row — is not a member of any project. That is the
/// fail-closed direction: legacy rows written before scope stamping existed
/// carry `None`, and treating them as "belongs to whichever room is asking"
/// would leak one room's board into another's.
#[must_use]
pub fn belongs_to_project(scope_id: Option<&str>, project_id: &str) -> bool {
    scope_id.and_then(project_id_of) == Some(project_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_scope_id_round_trips() {
        let rendered = project_scope_id("p-x7f2");
        assert_eq!(rendered, "project:p-x7f2");
        assert_eq!(project_id_of(&rendered), Some("p-x7f2"));
    }

    #[test]
    fn a_bare_prefix_names_no_project() {
        assert_eq!(project_id_of("project:"), None);
        assert!(!belongs_to_project(Some("project:"), ""));
    }

    #[test]
    fn other_scopes_are_not_projects() {
        assert_eq!(project_id_of(ORG), None);
        assert_eq!(project_id_of("personal:u-alice"), None);
        assert_eq!(
            project_id_of("p-x7f2"),
            None,
            "unprefixed is not a scope id"
        );
    }

    /// The membership predicate's two failure directions, which a `==` at the
    /// call site gets wrong in opposite ways: an unstamped row must not join a
    /// room, and a row from another room must not either.
    #[test]
    fn membership_is_exact_and_unstamped_rows_belong_nowhere() {
        assert!(belongs_to_project(Some("project:p-a"), "p-a"));
        assert!(!belongs_to_project(Some("project:p-b"), "p-a"));
        assert!(!belongs_to_project(Some("personal:u-alice"), "p-a"));
        assert!(!belongs_to_project(None, "p-a"));
    }

    /// A project id that itself contains the separator must not be truncated:
    /// `strip_prefix` + `split_once(':')` disagree here, and core's parser
    /// keeps everything after the FIRST colon.
    #[test]
    fn a_ref_containing_a_colon_survives_whole() {
        assert_eq!(project_id_of("project:p-a:b"), Some("p-a:b"));
    }
}
