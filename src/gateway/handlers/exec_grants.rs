//! Standing approval-grant RPCs — the surface that makes a grant *revocable*.
//!
//! - `exec.grants.list` — what has been allowed, at which scope, by whom.
//! - `exec.grant.revoke` — take one back.
//!
//! # Why this exists
//!
//! Until now a user who answered "allow for this session" could neither see the
//! grant nor take it back. The negative half of the same subsystem had an exit
//! — the denial breaker cools down and half-opens — and the positive half had
//! none, which is the asymmetry that makes a standing grant scarier than it
//! needs to be. A persistent ("always") grant without a revocation face would
//! be strictly worse: permanence you cannot enumerate is not a setting, it is a
//! leak.
//!
//! # Who may see and revoke what
//!
//! Shaped exactly like [`super::exec_approvals`], which this module is the
//! peacetime sibling of, and for the same reasons:
//!
//! * An unrestricted caller (operator, loopback, CLI) sees everything.
//! * A **member** sees only [`GrantScope::Session`] grants whose session is
//!   already visible to them (`visibility::session_visible`) — strictly less
//!   than the run they are permitted to start.
//! * A member sees **no** [`GrantScope::Always`] grant. They cannot create one
//!   (`exec::allowed_decisions::for_confirm_gate` never offers the tier to a
//!   non-operator turn), and enumerating them would list the operator's
//!   install-wide exceptions to somebody who cannot change them.
//! * A grant that is not addressable answers exactly as an unknown fingerprint
//!   does — one message, one code — so the id space cannot be probed.
//!
//! Revocation only ever *narrows* authority: the worst outcome of a revoke that
//! should not have happened is one extra approval card. That asymmetry is why
//! the write verb here is safe to carve open to members while the rest of the
//! `exec.` family stays operator-only.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::HandlerRegistry;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::sandbox::exec_approval::grants::{Grant, GrantStore};
use crate::sandbox::exec_approval::GrantScope;

/// Parameters for `exec.grant.revoke`.
#[derive(Debug, Deserialize)]
pub struct GrantRevokeParams {
    /// The action fingerprint, as returned by `exec.grants.list`.
    pub fingerprint: String,
    /// `"session"` or `"always"`. Required: the same action can hold a grant at
    /// both scopes, and guessing which one the user meant is how a revoke
    /// silently leaves the wider one standing.
    pub scope: String,
    /// The conversation a session-scoped grant belongs to. Ignored for
    /// `"always"`.
    #[serde(default)]
    pub session_key: Option<String>,
}

/// One row of `exec.grants.list`.
#[derive(Debug, Serialize)]
pub struct GrantView {
    pub fingerprint: String,
    pub tool: String,
    /// The redacted one-liner the human read on the card. This is what makes
    /// the list revocable by a person rather than by a hash.
    pub summary: String,
    pub scope: &'static str,
    pub granted_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

impl From<Grant> for GrantView {
    fn from(g: Grant) -> Self {
        Self {
            fingerprint: g.fingerprint,
            tool: g.tool,
            summary: g.summary,
            scope: g.scope.as_str(),
            granted_at_ms: g.granted_at_ms,
            granted_by: g.granted_by,
            session_key: g.session_key,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GrantListResponse {
    pub grants: Vec<GrantView>,
}

/// Register `exec.grants.list` / `exec.grant.revoke`.
///
/// `store` MUST be [`crate::sandbox::exec_approval::grants::global`] — the same
/// instance the confirm gate reads. A second instance would make every
/// revocation a no-op that reports success.
pub fn register_handlers(
    registry: &mut HandlerRegistry,
    store: Arc<GrantStore>,
    sessions: Arc<dyn SessionStore>,
) {
    {
        let g = store.clone();
        let s = sessions.clone();
        registry.register("exec.grants.list", move |req| {
            let g = g.clone();
            let s = s.clone();
            async move { handle_grants_list(req, g, s).await }
        });
    }
    {
        let g = store.clone();
        let s = sessions.clone();
        registry.register("exec.grant.revoke", move |req| {
            let g = g.clone();
            let s = s.clone();
            async move { handle_grant_revoke(req, g, s).await }
        });
    }
}

/// Whether this caller may be told the grant exists — and, therefore, may take
/// it back. See the module doc for the ruling behind each arm.
async fn grant_addressable_by_caller(sessions: &dyn SessionStore, grant: &Grant) -> bool {
    if !crate::gateway::caller_identity::caller_is_member() {
        return true;
    }
    // An install-wide grant is the operator's; a member could not have made it.
    if grant.scope == GrantScope::Always {
        return false;
    }
    let Some(raw) = grant.session_key.as_deref() else {
        return false;
    };
    let Some(key) = SessionKey::from_key_string(raw) else {
        return false;
    };
    matches!(
        sessions.get_metadata(&key).await,
        Ok(Some(meta)) if visibility::session_visible(&meta)
    )
}

async fn handle_grants_list(
    request: JsonRpcRequest,
    store: Arc<GrantStore>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let all = store.list();
    let visible = if crate::gateway::caller_identity::caller_is_member() {
        let mut kept = Vec::with_capacity(all.len());
        for grant in all {
            if grant_addressable_by_caller(&*sessions, &grant).await {
                kept.push(grant);
            }
        }
        kept
    } else {
        all
    };

    JsonRpcResponse::success(
        request.id,
        json!(GrantListResponse {
            grants: visible.into_iter().map(GrantView::from).collect(),
        }),
    )
}

async fn handle_grant_revoke(
    request: JsonRpcRequest,
    store: Arc<GrantStore>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: GrantRevokeParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(scope) = GrantScope::parse(&params.scope) else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Unknown grant scope: {}", params.scope),
        );
    };

    // Resolve the grant FIRST so the ownership question is asked about the
    // thing that actually exists, then revoke. `revoke` is idempotent, but a
    // revoke performed before the check would still tell a foreign caller that
    // the fingerprint was real.
    let target = store.list().into_iter().find(|g| {
        g.fingerprint == params.fingerprint
            && g.scope == scope
            && (scope == GrantScope::Always || g.session_key == params.session_key)
    });

    let revoked = match target {
        Some(ref grant) if grant_addressable_by_caller(&*sessions, grant).await => {
            match store.revoke(scope, grant.session_key.as_deref(), &grant.fingerprint) {
                Ok(done) => done,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to persist an approval-grant revocation");
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Could not write the grant registry: {e}"),
                    );
                }
            }
        }
        // Not there, or not yours — the SAME answer, so the fingerprint space
        // cannot be probed (`exec_approvals` makes the identical ruling).
        _ => false,
    };

    if revoked {
        JsonRpcResponse::success(request.id, json!({ "ok": true }))
    } else {
        JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Grant not found: {}", params.fingerprint),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::{CALLER_ROLE, CALLER_USER};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::sandbox::exec_approval::Grant as G;
    use serde_json::Value;
    use tempfile::TempDir;

    fn store() -> (TempDir, Arc<GrantStore>) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        (dir, Arc::new(GrantStore::with_path(path)))
    }

    /// Same fixture shape as [`super::super::exec_approvals`]'s: a real store,
    /// keyed by a real parseable `SessionKey`, because that is what a grant
    /// carries in production.
    fn sessions() -> (TempDir, Arc<dyn SessionStore>) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: tmp.path().to_path_buf(),
                ..Default::default()
            })
            .expect("file session store"),
        );
        (tmp, store)
    }

    async fn create_session(sessions: &Arc<dyn SessionStore>, key: &str, owner: &str) -> String {
        let parsed = SessionKey::from_key_string(key).expect("valid session_key fixture");
        let attribution = Some(crate::scope::ScopeAttribution::personal(owner));
        crate::scope::with_scope(attribution, sessions.get_or_create(&parsed))
            .await
            .expect("get_or_create");
        key.to_string()
    }

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    async fn as_member<F, T>(user: &str, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        CALLER_ROLE
            .scope(
                Some("member".to_string()),
                CALLER_USER.scope(Some(user.to_string()), fut),
            )
            .await
    }

    fn listed(resp: &JsonRpcResponse) -> Vec<String> {
        resp.result
            .as_ref()
            .and_then(|r| r.get("grants"))
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|g| g.get("fingerprint").and_then(Value::as_str))
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn session_grant(fp: &str, session: &str) -> G {
        G::new(fp, "bash", "bash: git status", GrantScope::Session)
            .in_session(Some(session.to_string()))
    }

    #[tokio::test]
    async fn the_list_names_both_tiers_for_an_operator() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        store.remember_session("agent:main:main", session_grant("fp-s", "agent:main:main"));
        store
            .remember_always(G::new("fp-a", "bash", "bash: ls", GrantScope::Always))
            .expect("persist");

        let resp = handle_grants_list(req("exec.grants.list", json!({})), store, sessions).await;
        let mut ids = listed(&resp);
        ids.sort();
        assert_eq!(ids, vec!["fp-a".to_string(), "fp-s".to_string()]);
    }

    /// The point of the surface: what a member granted, a member can see.
    #[tokio::test]
    async fn a_member_sees_their_own_session_grant() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let key = create_session(&sessions, "agent:main:mine", "u-alice").await;
        store.remember_session(&key, session_grant("fp-mine", &key));

        let resp = as_member(
            "u-alice",
            handle_grants_list(
                req("exec.grants.list", json!({})),
                store.clone(),
                sessions.clone(),
            ),
        )
        .await;
        assert_eq!(listed(&resp), vec!["fp-mine".to_string()]);
    }

    #[tokio::test]
    async fn a_member_sees_neither_a_foreign_grant_nor_an_install_wide_one() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let theirs = create_session(&sessions, "agent:main:theirs", "u-bob").await;
        store.remember_session(&theirs, session_grant("fp-theirs", &theirs));
        store
            .remember_always(G::new("fp-always", "bash", "bash: ls", GrantScope::Always))
            .expect("persist");

        let resp = as_member(
            "u-alice",
            handle_grants_list(
                req("exec.grants.list", json!({})),
                store.clone(),
                sessions.clone(),
            ),
        )
        .await;
        assert!(
            listed(&resp).is_empty(),
            "a member must see neither another member's grant nor the operator's install-wide one"
        );
    }

    #[tokio::test]
    async fn revoking_a_session_grant_makes_the_gate_ask_again() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let key = create_session(&sessions, "agent:main:mine", "u-alice").await;
        store.remember_session(&key, session_grant("fp-mine", &key));
        assert!(store.granted_within(Some(&key), "fp-mine", true).is_some());

        let resp = handle_grant_revoke(
            req(
                "exec.grant.revoke",
                json!({"fingerprint": "fp-mine", "scope": "session", "session_key": key}),
            ),
            store.clone(),
            sessions.clone(),
        )
        .await;
        assert!(resp.error.is_none(), "revoke failed: {:?}", resp.error);
        assert!(
            store.granted_within(Some(&key), "fp-mine", true).is_none(),
            "the store the gate reads is the store the RPC wrote"
        );
    }

    #[tokio::test]
    async fn revoking_a_persistent_grant_reaches_the_disk() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        store
            .remember_always(G::new("fp-a", "bash", "bash: ls", GrantScope::Always))
            .expect("persist");

        let resp = handle_grant_revoke(
            req(
                "exec.grant.revoke",
                json!({"fingerprint": "fp-a", "scope": "always"}),
            ),
            store.clone(),
            sessions,
        )
        .await;
        assert!(resp.error.is_none());
        assert!(store.granted_within(None, "fp-a", true).is_none());
        let reopened = GrantStore::with_path(store.path());
        assert!(
            reopened.granted_within(None, "fp-a", true).is_none(),
            "revocation persisted"
        );
    }

    /// A foreign grant is refused exactly as an unknown one is — the no-oracle
    /// ruling `exec_approvals` makes for approval ids.
    #[tokio::test]
    async fn a_foreign_grant_is_refused_exactly_as_an_unknown_one_is() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let theirs = create_session(&sessions, "agent:main:theirs", "u-bob").await;
        store.remember_session(&theirs, session_grant("fp-theirs", &theirs));

        let foreign = as_member(
            "u-alice",
            handle_grant_revoke(
                req(
                    "exec.grant.revoke",
                    json!({"fingerprint": "fp-theirs", "scope": "session", "session_key": theirs}),
                ),
                store.clone(),
                sessions.clone(),
            ),
        )
        .await;
        // The SAME request, once the grant genuinely does not exist. Comparing
        // two different fingerprints would only prove that the message quotes
        // its argument; what has to be indistinguishable is "yours is gone" and
        // "it was never yours".
        store
            .revoke(GrantScope::Session, Some(&theirs), "fp-theirs")
            .expect("revoke as the owner would");
        let unknown = as_member(
            "u-alice",
            handle_grant_revoke(
                req(
                    "exec.grant.revoke",
                    json!({"fingerprint": "fp-theirs", "scope": "session", "session_key": theirs}),
                ),
                store.clone(),
                sessions.clone(),
            ),
        )
        .await;
        assert_eq!(
            foreign.error.map(|e| (e.code, e.message)),
            unknown.error.map(|e| (e.code, e.message)),
            "'not yours' and 'not there' must be byte-identical"
        );
    }

    /// A member cannot revoke an install-wide grant they cannot see — the read
    /// and write faces answer from ONE predicate, not two.
    #[tokio::test]
    async fn a_member_cannot_revoke_a_persistent_grant() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        store
            .remember_always(G::new("fp-a", "bash", "bash: ls", GrantScope::Always))
            .expect("persist");

        let resp = as_member(
            "u-alice",
            handle_grant_revoke(
                req(
                    "exec.grant.revoke",
                    json!({"fingerprint": "fp-a", "scope": "always"}),
                ),
                store.clone(),
                sessions,
            ),
        )
        .await;
        assert!(resp.error.is_some());
        assert!(
            store.granted_within(None, "fp-a", true).is_some(),
            "still standing"
        );
    }

    #[tokio::test]
    async fn an_unknown_scope_is_refused_not_guessed() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let resp = handle_grant_revoke(
            req(
                "exec.grant.revoke",
                json!({"fingerprint": "fp-a", "scope": "forever"}),
            ),
            store,
            sessions,
        )
        .await;
        assert!(resp.error.is_some());
    }

    #[test]
    fn register_handlers_registers_all_methods() {
        let (_d, store) = store();
        let (_sd, sessions) = sessions();
        let mut registry = HandlerRegistry::new();
        register_handlers(&mut registry, store, sessions);
        for m in ["exec.grants.list", "exec.grant.revoke"] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
