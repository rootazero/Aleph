//! `projects.channel.*` — bind a channel group conversation to a project room.
//!
//! 把一个频道群会话绑定到一个项目房间。绑定之后，名册成员在群里说话，
//! agent 就从房间的共享记忆 / 笔记 / 工作区作答。
//!
//! # Why `bind` and `unbind` are operator-only
//!
//! The exposure runs OUTWARD. After binding, a roster member speaking in the
//! group makes the agent answer from the room's shared memory, notes and
//! workspace — and that answer is delivered to the whole conversation,
//! including people the roster does not control. That is the point of the
//! feature, and it is also why the decision belongs to an operator rather than
//! to a room owner who may be an ordinary member.
//!
//! There is a second, independently sufficient reason. The binding table is
//! `PRIMARY KEY (channel_id, peer_kind, peer_id)`, because one conversation
//! cannot belong to two rooms without making
//! [`ProjectStore::project_for_conversation`] ambiguous. So
//! [`ProjectStore::bind_conversation`]'s conflict arm returns a message
//! **naming the owning project id** — an existence-and-identity oracle, and it
//! is acceptable only because the sole caller who reaches it is an operator,
//! who is entitled to know every project. The uniqueness constraint and the
//! admin gate are one design, not two: widening who may call `bind` reopens
//! that leak.
//!
//! `bind` and `unbind` are therefore in [`method_admin::ADMIN_METHODS`];
//! `list` stays open and is narrowed to the roster by [`gate_project`], so a
//! member can see where their room lives without being able to move it.
//!
//! The admin gate lives upstream, at `process_request`'s single chokepoint —
//! these handlers do not re-derive it, for the same reason no `projects.*`
//! handler writes `project.owner_user_id == caller` at its own call site.
//!
//! [`method_admin::ADMIN_METHODS`]: crate::gateway::method_admin
//! [`ProjectStore::project_for_conversation`]: crate::projects::ProjectStore::project_for_conversation
//! [`ProjectStore::bind_conversation`]: crate::projects::ProjectStore::bind_conversation

use std::sync::Arc;

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::parse_params;
use super::projects::{gate_project, project_error_response};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::ChangeKind;
use crate::gateway::session_store::SessionStore;
use crate::projects::{binding, ChannelBinding, ProjectStore};
use aleph_protocol::projects::{
    ChannelBindParams, ChannelBindResult, ChannelBindingRow, ChannelListParams, ChannelListResult,
    ChannelUnbindParams, ChannelUnbindResult, RescopeOutcome,
};

/// Project a stored binding onto the wire row.
///
/// Field-by-field rather than `#[derive]`-shared, because the stored
/// [`ChannelBinding`] and the wire [`ChannelBindingRow`] are allowed to
/// diverge — the stored one documents which of its components are normalized,
/// which is a storage fact and not something a client needs.
fn row(b: ChannelBinding) -> ChannelBindingRow {
    ChannelBindingRow {
        project_id: b.project_id,
        channel_id: b.channel_id,
        peer_kind: b.peer_kind,
        peer_id: b.peer_id,
        bound_by: b.bound_by,
        bound_at: b.bound_at,
        label: b.label,
    }
}

/// Move the conversation's existing transcript into the room's scope, and say
/// which of the three things actually happened.
///
/// # Why this exists at all
///
/// Without it a bound conversation splits in two: the RUN takes the room scope
/// (memory partition, roster, room context) while the ROW keeps
/// `personal:<first speaker>`, so every other member's `session_visible_to`
/// says false and the group stays invisible in their session list. The binding
/// would look correct on every surface and be useless on all but one.
async fn rescope_existing_transcript(
    sessions: &dyn SessionStore,
    params: &ChannelBindParams,
    project_id: &str,
) -> RescopeOutcome {
    // `SessionKey::group` takes the ROUTING `PeerKind`; `ProjectStore`'s
    // binding methods take the WIRE `BindingPeerKind` and are called with
    // `params.peer_kind` unconverted. This is the one place the conversion is
    // needed, and `binding::from_wire` is its only author.
    //
    // `SessionKey::group` normalizes `channel` and `peer_id` through the same
    // `sanitize_component` that `binding::normalize_component` calls, so
    // passing the operator's raw spelling here lands on the same key a live
    // inbound message produces.
    let key = crate::routing::session_key::SessionKey::group(
        default_conversation_agent_id(),
        &params.channel_id,
        binding::from_wire(params.peer_kind),
        &params.peer_id,
    );
    classify_rescope(
        sessions.rescope_attribution(&key, project_id).await,
        params,
        project_id,
    )
}

/// Turn [`SessionStore::rescope_attribution`]'s three outcomes into the three
/// the receipt carries — and log the third.
///
/// Split from the `await` above so the mapping is testable without a
/// thirty-method stub store: neither shipped backend can be made to return
/// `Err` on demand, and a hand-written stub that could would be a second
/// derivation of the thing under test. This function IS the production
/// mapping, called with the production arguments.
///
/// `Err` must not fold into `Ok(false)`. `RescopeOutcome::NothingToMove` is
/// rendered by clients as "nobody has spoken in that conversation yet", which
/// would be a confident factual claim about a conversation whose store just
/// failed. The bind itself has already committed by the time this runs, so an
/// error must not fail the RPC — but a swallowed error with no log is how
/// "something will heal it later" becomes "nobody ever knew", hence the
/// `warn!`.
fn classify_rescope(
    result: Result<bool, crate::gateway::session_store::error::SessionStoreError>,
    params: &ChannelBindParams,
    project_id: &str,
) -> RescopeOutcome {
    match result {
        Ok(true) => RescopeOutcome::Moved,
        Ok(false) => RescopeOutcome::NothingToMove,
        Err(e) => {
            tracing::warn!(
                error = %e,
                channel_id = %params.channel_id,
                peer_id = %params.peer_id,
                project_id = %project_id,
                "projects.channel.bind: binding recorded, but the session store could not \
                 report whether an existing transcript moved"
            );
            RescopeOutcome::Unknown
        }
    }
}

/// The agent id a bound conversation's session row is keyed under.
///
/// # This is a known approximation, deliberately isolated here
///
/// A binding is keyed on the CONVERSATION and carries no agent id on purpose —
/// see `projects::binding`'s `the_agent_id_is_not_part_of_the_conversation`:
/// an `agent_switch` must not silently un-bind a room. But
/// [`SessionKey::group`] does carry one, so addressing the conversation's
/// existing row forces this handler to name an agent the binding does not have.
///
/// `"main"` is the value [`crate::gateway::router::AgentRouter`] and
/// `AgentInstanceManager` both default to, so it is correct on every
/// deployment that has not repointed the channel. On one that has
/// (`channels.set_agent`, a route binding),
/// `inbound_router::agent_resolver::resolve_session_key_with_agent` built the
/// row under THAT agent id and this lookup misses it: the row keeps
/// `personal:<first speaker>` and the receipt reports
/// [`RescopeOutcome::NothingToMove`].
///
/// It is a function rather than a literal so the miss has one address to be
/// fixed at, and so the reason survives next to the value.
///
/// [`SessionKey::group`]: crate::routing::session_key::SessionKey::group
const fn default_conversation_agent_id() -> &'static str {
    "main"
}

/// `projects.channel.bind` — point a room at a channel conversation.
///
/// Operator-only; see the module doc for both reasons.
pub async fn handle_bind(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    sessions: Arc<dyn SessionStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: ChannelBindParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // `params.peer_kind` needs no hand-validation: it is a typed
    // `BindingPeerKind`, so `parse_params` already rejected every other
    // spelling at the deserialization boundary (`aleph_protocol::projects`
    // pins that with `an_uppercase_peer_kind_is_rejected_at_the_parse_boundary`).
    // A manual re-check here would be a branch that can never fire, plus a
    // second author for the accepted spellings.

    // Visibility first, exactly like every other addressed `projects.*` verb:
    // a room the caller cannot see must be indistinguishable from one that
    // does not exist.
    let project = match gate_project(&store, request.id.clone(), &params.project_id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };

    let actor = crate::gateway::caller_identity::current_caller_user();
    // `params.peer_kind` goes STRAIGHT through: the store's binding methods
    // take `aleph_protocol::projects::BindingPeerKind`, the same type the wire
    // carries. Only `SessionKey::group` needs `binding::from_wire`.
    let binding = match store.bind_conversation(
        &project.id,
        &params.channel_id,
        params.peer_kind,
        &params.peer_id,
        actor.as_deref(),
        params.label.as_deref(),
    ) {
        Ok(b) => b,
        // One classifier for `ProjectError`, shared with every other
        // `projects.*` handler — a second `match` here would be a second
        // author for which failures are the caller's and which are ours.
        Err(e) => return project_error_response(request.id, e),
    };

    let rescoped = rescope_existing_transcript(sessions.as_ref(), &params, &project.id).await;

    if let Some(log) = crate::security::audit::global() {
        log.log(crate::security::audit::AuditEntry::authority_change(
            actor,
            // The NORMALIZED components, not the operator's raw spelling: this
            // record has to name the key that actually governs routing from
            // now on. The raw spelling is preserved in the binding's `label`.
            format!(
                "projects.channel.bind: {}:{} → {} (rescoped_session={rescoped})",
                binding.channel_id, binding.peer_id, project.id
            ),
        ));
    }
    crate::projects::events::publish_changed(&event_bus, &project.id, ChangeKind::Updated, None);

    JsonRpcResponse::success(
        request.id,
        json!(ChannelBindResult {
            binding: row(binding),
            rescoped_session: rescoped,
        }),
    )
}

/// `projects.channel.unbind` — release a conversation.
///
/// Operator-only; see the module doc.
///
/// # Unbinding does not move the transcript back — and that is deliberate
///
/// Once a session row's `scope_id` is `project:<pid>`, nothing moves it:
/// `stamp_attribution` is create-only, `backfill_attribution` early-returns
/// when either field is already set, and
/// [`SessionStore::rescope_attribution`] — the only other writer — is called
/// from [`handle_bind`] and nowhere else. So after `bind` then `unbind`, the
/// conversation is no longer bound but its transcript **stays in the room's
/// scope permanently**: still visible to the roster, still in the room's
/// memory partition.
///
/// This is the right behaviour and must not be "fixed". There is no correct
/// destination to move it back to — the previous scope was never recorded, and
/// reverting to `personal:<somebody>` means picking a person, where picking
/// wrong is worse than not reverting. The roster also genuinely participated
/// in that transcript; yanking it out would remove content people contributed
/// to.
///
/// What would be wrong is silence about it. The receipt reports only that the
/// binding is gone, so an operator will reasonably assume symmetry that does
/// not exist. The statement is unconditionally true — it holds whether or not
/// a transcript exists — so it is client copy rather than wire state, and it
/// belongs in every surface's `unbind` output:
///
/// > The conversation's existing transcript stays with the room — unbinding
/// > stops future turns from joining it, it does not move history back.
///
/// 解绑不会把已有记录搬回去：这是有意的，因为没有一个正确的目的地可搬。
/// 每个客户端面都必须把上面那句话打印出来。
///
/// [`SessionStore::rescope_attribution`]: crate::gateway::session_store::SessionStore::rescope_attribution
pub async fn handle_unbind(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: ChannelUnbindParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Read the owning room BEFORE the delete so the audit line and the
    // `projects.changed` frame can name it: answering "whose binding was that"
    // must not depend on whether the delete kept a record.
    //
    // A store error here is NOT folded into "unbound" — it only costs the
    // audit line its subject, so it is logged and the unbind proceeds.
    let owner =
        match store.project_for_conversation(&params.channel_id, params.peer_kind, &params.peer_id)
        {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    channel_id = %params.channel_id,
                    peer_id = %params.peer_id,
                    "projects.channel.unbind: could not read the owning room before unbinding; \
                     the audit record and the projects.changed frame will not name it"
                );
                None
            }
        };
    let unbound =
        match store.unbind_conversation(&params.channel_id, params.peer_kind, &params.peer_id) {
            Ok(v) => v,
            Err(e) => return project_error_response(request.id, e),
        };
    if unbound {
        if let Some(log) = crate::security::audit::global() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                crate::gateway::caller_identity::current_caller_user(),
                format!(
                    "projects.channel.unbind: {}:{} (was {})",
                    params.channel_id,
                    params.peer_id,
                    // "unknown" rather than "unbound": we deleted a row, so it
                    // WAS bound — this arm means the pre-read failed, and
                    // saying "unbound" here would assert the opposite of what
                    // just happened.
                    owner.as_deref().unwrap_or("unknown"),
                ),
            ));
        }
        if let Some(pid) = owner.as_deref() {
            crate::projects::events::publish_changed(&event_bus, pid, ChangeKind::Updated, None);
        }
    }
    JsonRpcResponse::success(request.id, json!(ChannelUnbindResult { unbound }))
}

/// `projects.channel.list` — open, narrowed to the roster by [`gate_project`].
pub async fn handle_list(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: ChannelListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.project_id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    match store.bindings_for(&project.id) {
        Ok(bs) => JsonRpcResponse::success(
            request.id,
            json!(ChannelListResult {
                bindings: bs.into_iter().map(row).collect()
            }),
        ),
        // Deliberately no `NotFound` arm: `bindings_for` cannot produce one
        // (it is a plain `SELECT ... WHERE project_id = ?`, so a room that
        // vanished reads as an empty list, not an error), and `gate_project`
        // above already answers unreachability with the shared not-found
        // shape. An arm here would be a branch that can never fire.
        Err(e) => project_error_response(request.id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::error::SessionStoreError;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::projects::roster::TEST_GUARD as ROSTER_TEST_GUARD;
    use crate::routing::session_key::{PeerKind, SessionKey};
    use crate::scope::{with_scope, ScopeAttribution};
    use aleph_protocol::projects::BindingPeerKind;
    use rusqlite::Connection;
    use serde_json::Value;
    use std::sync::MutexGuard;

    /// `u-alice` owns the room and `u-bob` is on its roster; `u-mallory` is a
    /// stranger. The returned guard serialises the roster projection — see
    /// [`crate::projects::roster::TEST_GUARD`].
    fn room() -> (
        Arc<ProjectStore>,
        crate::projects::Project,
        MutexGuard<'static, ()>,
    ) {
        let guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = Arc::new(ProjectStore::new(Connection::open_in_memory().unwrap()));
        store.create_schema().unwrap();
        let project = store.create("shared room", Some("u-alice"), None).unwrap();
        store.add_member(&project.id, "u-bob").unwrap();
        (store, project, guard)
    }

    /// A real backend, not a stub: `Moved` and `NothingToMove` are then the
    /// store's own answers rather than a fixture's idea of them.
    fn sessions() -> (Arc<dyn SessionStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        })
        .expect("file session store");
        (Arc::new(store), dir)
    }

    fn bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    fn rpc(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    fn err_of(resp: &JsonRpcResponse) -> (i32, String) {
        let e = resp.error.as_ref().expect("expected an error response");
        (e.code, e.message.clone())
    }

    fn bind_params(project_id: &str, peer_id: &str) -> Value {
        json!({
            "project_id": project_id,
            "channel_id": "telegram",
            "peer_kind": "group",
            "peer_id": peer_id,
        })
    }

    /// Ruling AG, at the one place it is decided. `Err` must be its own answer:
    /// folding it into `Ok(false)` makes every client print "nobody has spoken
    /// in that conversation yet" about a conversation whose store just failed.
    ///
    /// This calls the production mapping directly. Neither shipped backend can
    /// be made to return `Err` on demand, and a stub that could would be a
    /// second derivation of the thing under test.
    #[test]
    fn a_store_error_is_reported_as_unknown_and_not_as_nothing_to_move() {
        let params: ChannelBindParams =
            serde_json::from_value(bind_params("p-1", "c1")).expect("params");
        let moved = classify_rescope(Ok(true), &params, "p-1");
        let nothing = classify_rescope(Ok(false), &params, "p-1");
        let failed = classify_rescope(Err(SessionStoreError::Unsupported), &params, "p-1");

        assert_eq!(moved, RescopeOutcome::Moved);
        assert_eq!(nothing, RescopeOutcome::NothingToMove);
        assert_eq!(failed, RescopeOutcome::Unknown);
        assert_ne!(
            failed, nothing,
            "a store that errored has not found nothing to move — a client \
             rendering the two identically asserts a result it never saw"
        );
    }

    /// The row half of a bind. Without it the run takes the room scope while
    /// the row keeps `personal:<first speaker>`, so the group stays invisible
    /// in every other member's session list.
    #[tokio::test]
    async fn binding_moves_the_conversations_existing_transcript_into_the_room() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        // u-alice speaks in the group before it is ever bound: the row is
        // created under HER personal scope, which is the split to be closed.
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                ),
            )
            .await;

        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);
        assert_eq!(result.binding.project_id, project.id);
        assert_eq!(
            result.binding.peer_id, "c1",
            "the stored key is normalized the same way a live SessionKey is"
        );

        let meta = sessions.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(
            meta.scope_id.as_deref(),
            Some(format!("project:{}", project.id).as_str()),
            "the receipt said Moved, so the row must actually be in the room scope"
        );
        assert_eq!(
            meta.owner_user_id.as_deref(),
            Some("u-alice"),
            "the byline still names whoever spoke first"
        );
    }

    /// The common case for a freshly bound group, and the one the `Unknown`
    /// arm must stay distinguishable from.
    #[tokio::test]
    async fn binding_a_conversation_nobody_has_spoken_in_reports_nothing_to_move() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc(
                        "projects.channel.bind",
                        bind_params(&project.id, "C-unspoken"),
                    ),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                ),
            )
            .await;

        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::NothingToMove);
    }

    /// The no-oracle contract: a room the caller is not on the roster of must
    /// be byte-for-byte indistinguishable from an id that was never minted.
    /// If the refusal reads differently, the refusal itself tells a stranger
    /// the room is real.
    #[tokio::test]
    async fn a_non_member_cannot_list_a_rooms_bindings() {
        let (store, project, _guard) = room();
        store
            .bind_conversation(
                &project.id,
                "telegram",
                BindingPeerKind::Group,
                "C1",
                Some("u-alice"),
                None,
            )
            .unwrap();

        let refused = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_list(
                    rpc("projects.channel.list", json!({ "project_id": project.id })),
                    store.clone(),
                ),
            )
            .await;
        let missing = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_list(
                    rpc(
                        "projects.channel.list",
                        json!({ "project_id": "p-never-minted" }),
                    ),
                    store.clone(),
                ),
            )
            .await;

        let (refused_code, refused_msg) = err_of(&refused);
        let (missing_code, _) = err_of(&missing);
        assert_eq!(refused_code, missing_code);
        assert_eq!(
            refused_msg,
            format!("project not found: {}", project.id),
            "the refusal must be the not-found wording, never a permission denial"
        );
        assert!(
            !refused_msg.contains("telegram") && !refused_msg.contains("c1"),
            "a refusal must not leak what is bound: {refused_msg}"
        );
    }

    /// A roster member sees where their own room lives. `list` is deliberately
    /// NOT admin-gated — being able to see a binding is not being able to move
    /// one.
    #[tokio::test]
    async fn a_roster_member_can_list_the_rooms_bindings() {
        let (store, project, _guard) = room();
        store
            .bind_conversation(
                &project.id,
                "telegram",
                BindingPeerKind::Group,
                "C1",
                Some("u-alice"),
                Some("Eng standup"),
            )
            .unwrap();

        let resp = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list(
                    rpc("projects.channel.list", json!({ "project_id": project.id })),
                    store.clone(),
                ),
            )
            .await;
        let listed: ChannelListResult =
            serde_json::from_value(resp.result.expect("a member may list")).expect("list result");
        assert_eq!(listed.bindings.len(), 1);
        assert_eq!(listed.bindings[0].peer_kind, BindingPeerKind::Group);
        assert_eq!(listed.bindings[0].label.as_deref(), Some("Eng standup"));
    }

    /// `bind` runs the same admission gate as every other addressed
    /// `projects.*` verb. The admin gate upstream decides WHO may call the
    /// method; this decides which rooms they may name once they can.
    #[tokio::test]
    async fn a_non_member_cannot_bind_a_conversation_to_a_room() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        let refused = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions,
                    bus(),
                ),
            )
            .await;
        let (_, msg) = err_of(&refused);
        assert_eq!(msg, format!("project not found: {}", project.id));
        assert_eq!(
            store
                .project_for_conversation("telegram", BindingPeerKind::Group, "C1")
                .unwrap(),
            None,
            "a refused bind must not have written a row"
        );
    }

    /// A conversation belongs to at most one room — the binding table PRIMARY
    /// KEY says so, and an overwrite would move a live room traffic somewhere
    /// its members cannot see. The refusal is a caller-fixable
    /// `INVALID_PARAMS`, not an `INTERNAL_ERROR`: changing the request fixes
    /// it, so telling the operator to go read server logs would be wrong.
    #[tokio::test]
    async fn a_conversation_already_bound_elsewhere_is_refused_not_taken_over() {
        let (store, first, _guard) = room();
        let (sessions, _dir) = sessions();
        let second = store.create("other room", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &first.id,
                "telegram",
                BindingPeerKind::Group,
                "C1",
                Some("u-alice"),
                None,
            )
            .unwrap();

        let refused = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&second.id, "C1")),
                    store.clone(),
                    sessions,
                    bus(),
                ),
            )
            .await;
        let (code, msg) = err_of(&refused);
        assert_eq!(code, crate::gateway::protocol::INVALID_PARAMS);
        assert!(
            msg.contains(&first.id),
            "the refusal names the owner: {msg}"
        );
        assert_eq!(
            store
                .project_for_conversation("telegram", BindingPeerKind::Group, "C1")
                .unwrap()
                .as_deref(),
            Some(first.id.as_str()),
            "the first room keeps it"
        );
    }

    /// `Ok(false)` from the store means nothing was bound, and the receipt has
    /// to say that rather than report a release that did not happen.
    #[tokio::test]
    async fn unbinding_a_conversation_that_was_never_bound_reports_false() {
        let (store, _project, _guard) = room();
        let resp = handle_unbind(
            rpc(
                "projects.channel.unbind",
                json!({ "channel_id": "telegram", "peer_kind": "group", "peer_id": "C-nope" }),
            ),
            store.clone(),
            bus(),
        )
        .await;
        let result: ChannelUnbindResult =
            serde_json::from_value(resp.result.expect("unbind succeeds")).expect("unbind result");
        assert!(!result.unbound);
    }

    /// Ruling AI, pinned so the doc comment has a test. Unbinding stops FUTURE
    /// turns from joining the room; it does not move history back, because
    /// there is no correct destination to move it to — the previous scope was
    /// never recorded, and reverting to `personal:<somebody>` means picking a
    /// person, where picking wrong is worse than not reverting.
    ///
    /// If this ever fails because somebody taught `unbind` to revert, that is
    /// a decision to re-open with a human — not a test to update.
    #[tokio::test]
    async fn unbinding_leaves_the_existing_transcript_in_the_rooms_scope() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                ),
            )
            .await;
        let resp = handle_unbind(
            rpc(
                "projects.channel.unbind",
                json!({ "channel_id": "telegram", "peer_kind": "group", "peer_id": "C1" }),
            ),
            store.clone(),
            bus(),
        )
        .await;
        let result: ChannelUnbindResult =
            serde_json::from_value(resp.result.expect("unbind succeeds")).expect("unbind result");
        assert!(result.unbound);

        let meta = sessions.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(
            meta.scope_id.as_deref(),
            Some(format!("project:{}", project.id).as_str()),
            "unbinding stops future turns from joining the room; it does not move \
             history back. Every client unbind receipt must say so."
        );
    }

    /// A misspelled `peer_kind` must be rejected at the parse boundary rather
    /// than stored as a second, never-matched row under the
    /// `(channel_id, peer_kind, peer_id)` primary key. The handler writes no
    /// validator for this on purpose — the typed wire field is the validator,
    /// and this is the assertion that the handler really inherits it.
    #[tokio::test]
    async fn a_misspelled_peer_kind_never_reaches_the_store() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();
        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc(
                        "projects.channel.bind",
                        json!({
                            "project_id": project.id,
                            "channel_id": "telegram",
                            "peer_kind": "Group",
                            "peer_id": "C1",
                        }),
                    ),
                    store.clone(),
                    sessions,
                    bus(),
                ),
            )
            .await;
        let (code, _) = err_of(&resp);
        assert_eq!(code, crate::gateway::protocol::INVALID_PARAMS);
        assert!(
            store.bindings_for(&project.id).unwrap().is_empty(),
            "nothing may be written for a spelling the wire does not accept"
        );
    }
}
