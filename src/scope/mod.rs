//! Scope vocabulary and ambient attribution for data isolation.
//!
//! This module implements the spec §5.1 vocabulary for scope taxonomy:
//! - `Org`: organization-wide scope
//! - `Personal(String)`: personal scope for a user (ref format: `u-<uuid>` from users.rs:162)
//! - `Project(String)`: project scope (ref format: `p-…` reserved for P2)
//!
//! The `Personal` and `Project` refs are the P0 id formats verbatim, so
//! `partition_suffix` composes directly with `project_scope::scoped_agent_id`.
//! The three suffix families — `proj-*` (legacy directory feature), `u-*` (personal),
//! `p-*` (project) — are siblings, never nested.
//!
//! The task-local follows `src/projects/run_context.rs`'s contract verbatim:
//! children spawned via `tokio::spawn` MUST capture the attribution before the
//! spawn boundary (use [`current_scope`] at the call site, then feed the captured
//! `ScopeAttribution` back into the new request). Synchronous await chains inherit
//! the scope automatically.
//!
//! Metadata round-tripping via [`stamp_metadata`] and [`scope_from_metadata`]
//! requires BOTH keys to be present and coherent; if either is missing or invalid,
//! `scope_from_metadata` returns `None` (fail-closed, legacy compat).

use std::collections::HashMap;

/// A scope identifier representing the visibility boundary for an agent or resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeId {
    /// Organization-wide scope; visible to all members.
    Org,
    /// Personal scope for a specific user. Ref is the user ID (e.g., `u-alice`).
    Personal(String),
    /// Project scope. Ref is the project ID (e.g., `p-x7f2`).
    Project(String),
}

impl ScopeId {
    /// Render the scope to its canonical string form.
    /// - `Org` → `"org"`
    /// - `Personal(u)` → `"personal:<u>"`
    /// - `Project(p)` → `"project:<p>"`
    pub fn render(&self) -> String {
        match self {
            ScopeId::Org => "org".to_string(),
            ScopeId::Personal(ref_id) => format!("personal:{}", ref_id),
            ScopeId::Project(ref_id) => format!("project:{}", ref_id),
        }
    }

    /// Parse a scope from its rendered form. Returns `None` if the input is
    /// invalid or refers to an unknown scope kind (fail-closed).
    pub fn parse(s: &str) -> Option<ScopeId> {
        match s {
            "org" => Some(ScopeId::Org),
            _ => {
                if let Some((kind, ref_id)) = s.split_once(':') {
                    match kind {
                        "personal" if !ref_id.is_empty() => {
                            Some(ScopeId::Personal(ref_id.to_string()))
                        }
                        "project" if !ref_id.is_empty() => {
                            Some(ScopeId::Project(ref_id.to_string()))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Extract the partition suffix (the ref) from this scope.
    /// - `Org` → `None`
    /// - `Personal(u)` → `Some(u)`
    /// - `Project(p)` → `Some(p)`
    pub fn partition_suffix(&self) -> Option<&str> {
        match self {
            ScopeId::Org => None,
            ScopeId::Personal(ref_id) => Some(ref_id),
            ScopeId::Project(ref_id) => Some(ref_id),
        }
    }
}

/// Ambient scope attribution: combines the owning user ID and the scope boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeAttribution {
    /// The user ID of the owner (e.g., `u-alice`).
    pub owner_user_id: String,
    /// The scope boundary (org, personal, or project).
    pub scope: ScopeId,
}

impl ScopeAttribution {
    /// Create a personal scope attribution for the given user.
    pub fn personal(user_id: &str) -> Self {
        ScopeAttribution {
            owner_user_id: user_id.to_string(),
            scope: ScopeId::Personal(user_id.to_string()),
        }
    }
}

/// Metadata key for the owning user ID.
pub const OWNER_META_KEY: &str = "scope_owner_user_id";

/// Metadata key for the scope ID (rendered form).
pub const SCOPE_META_KEY: &str = "scope_id";

tokio::task_local! {
    static CURRENT_ATTRIBUTION: Option<ScopeAttribution>;
}

/// Run `fut` with the given scope attribution visible to [`current_scope`] for
/// the lifetime of the future. The scope is bounded by the future, so once it
/// resolves the task-local goes back to whatever the parent stack had.
pub async fn with_scope<F, T>(attr: Option<ScopeAttribution>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_ATTRIBUTION.scope(attr, fut).await
}

/// Read the active scope attribution, if any. Returns `None` outside a
/// [`with_scope`] scope or when the surrounding scope explicitly set `None`.
#[must_use]
pub fn current_scope() -> Option<ScopeAttribution> {
    CURRENT_ATTRIBUTION
        .try_with(|attr| attr.clone())
        .ok()
        .flatten()
}

/// Reconstruct a `ScopeAttribution` from metadata.
/// Requires BOTH `OWNER_META_KEY` and `SCOPE_META_KEY` to be present and
/// coherent; returns `None` if either is missing or the scope fails to parse
/// (fail-closed, for legacy compat).
pub fn scope_from_metadata(meta: &HashMap<String, String>) -> Option<ScopeAttribution> {
    let owner_user_id = meta.get(OWNER_META_KEY)?.clone();
    let scope_str = meta.get(SCOPE_META_KEY)?;
    let scope = ScopeId::parse(scope_str)?;
    Some(ScopeAttribution {
        owner_user_id,
        scope,
    })
}

/// Write a `ScopeAttribution` into metadata as key-value pairs.
pub fn stamp_metadata(meta: &mut HashMap<String, String>, attr: &ScopeAttribution) {
    meta.insert(OWNER_META_KEY.to_string(), attr.owner_user_id.clone());
    meta.insert(SCOPE_META_KEY.to_string(), attr.scope.render());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parse_round_trips_all_three_kinds() {
        for s in [
            ScopeId::Org,
            ScopeId::Personal("u-alice".into()),
            ScopeId::Project("p-x7f2".into()),
        ] {
            assert_eq!(ScopeId::parse(&s.render()), Some(s.clone()));
        }
        assert_eq!(ScopeId::parse("personal:"), None, "empty ref is invalid");
        assert_eq!(
            ScopeId::parse("group:x"),
            None,
            "unknown kind is invalid — fail closed"
        );
    }

    #[test]
    fn partition_suffix_is_the_ref_verbatim() {
        assert_eq!(ScopeId::Org.partition_suffix(), None);
        assert_eq!(
            ScopeId::Personal("u-alice".into()).partition_suffix(),
            Some("u-alice")
        );
        assert_eq!(
            ScopeId::Project("p-x7f2".into()).partition_suffix(),
            Some("p-x7f2")
        );
    }

    #[tokio::test]
    async fn task_local_scopes_and_does_not_cross_spawn() {
        // mirror src/projects/run_context.rs::task_local_does_not_cross_spawn_boundary
        let attr = ScopeAttribution::personal("u-alice");
        with_scope(Some(attr), async {
            assert_eq!(current_scope().unwrap().owner_user_id, "u-alice");
            let handle = tokio::spawn(async { current_scope() });
            assert!(
                handle.await.unwrap().is_none(),
                "task-locals must not cross spawn"
            );
        })
        .await;
        assert!(current_scope().is_none(), "scope pops on future completion");
    }

    #[test]
    fn metadata_round_trip() {
        let mut m = HashMap::new();
        stamp_metadata(&mut m, &ScopeAttribution::personal("u-alice"));
        let back = scope_from_metadata(&m).unwrap();
        assert_eq!(back.owner_user_id, "u-alice");
        assert_eq!(back.scope, ScopeId::Personal("u-alice".into()));
        assert!(
            scope_from_metadata(&HashMap::new()).is_none(),
            "absent keys → None (legacy)"
        );
        // Corrupt scope_id with a present owner: fail closed to None, never guess.
        let mut bad = HashMap::new();
        bad.insert(OWNER_META_KEY.to_string(), "u-alice".into());
        bad.insert(SCOPE_META_KEY.to_string(), "garbage".into());
        assert!(scope_from_metadata(&bad).is_none());
    }
}
