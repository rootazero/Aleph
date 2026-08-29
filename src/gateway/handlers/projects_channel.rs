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
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::projects::{binding, ChannelBinding, ProjectStore};
use crate::routing::session_key::SessionKey;
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
///
/// # Why it enumerates instead of addressing one key (Ruling AM)
///
/// [`SessionKey::Group`] carries an `agent_id`; a binding, by design, does not
/// — `binding::conversation_of` drops it, and
/// `the_agent_id_is_not_part_of_the_conversation` pins that an `agent_switch`
/// must not silently un-bind a room. So there is no correct single key to
/// build here: any agent id this function named would be a guess, differing
/// only in how often it happened to be right. Naming `"main"` would miss every
/// deployment that repointed the channel (`channels.set_agent`, a route
/// binding), leaving that row `personal:<first speaker>` while the receipt
/// said `NothingToMove` — a confident claim about a conversation with a full
/// transcript.
///
/// Enumerating removes the question rather than answering it: every row whose
/// key decomposes to THIS conversation is moved, whichever agent served it.
/// That also handles the case a single key cannot express at all — two agents
/// having served the same group over its lifetime, which is two rows.
///
/// The generalisation, for whoever meets this shape next: **when a construction
/// forces you to invent a field, look upstream for the projection that dropped
/// it. The fix is to stop requiring the field, not to guess it better.**
async fn rescope_existing_transcript(
    sessions: &dyn SessionStore,
    bound: &ChannelBinding,
) -> RescopeOutcome {
    // `bound` rather than the raw `ChannelBindParams`: `bind_conversation`
    // already normalized `channel_id` / `peer_id` through
    // `binding::normalize_component`, and those normalized components are
    // exactly what `conversation_of` reports for a live key. Comparing against
    // the stored row removes any second opinion about how normalization works.
    let target = (
        bound.channel_id.as_str(),
        bound.peer_kind,
        bound.peer_id.as_str(),
    );

    // ⚠️ `owner_visible_to` is deliberately `None`, and that is a FUNCTIONAL
    // REQUIREMENT rather than a missing gate.
    //
    // The rows being searched for may belong to ANYBODY: `personal:<first
    // speaker>` is precisely the class this verb exists to move. Adding
    // `visible_owner_filter()` here — the reflex, and correct almost
    // everywhere else in this crate — would silently drop exactly the rows the
    // bind is supposed to relocate, and the receipt would still say it
    // succeeded.
    //
    // The admission decision was already made upstream: `bind` is admin-gated
    // in `method_admin::ADMIN_METHODS`, and `gate_project` has already refused
    // a room the caller cannot see. An operator entitled to bind the room is
    // entitled to move its conversation's rows.
    //
    // This is the mirror image of the repo's `..Default::default()` criterion,
    // where leaving this same field `None` was the DEFECT (`session_list`
    // showing every owner's rows). Same field, opposite direction: there the
    // filter was the point, here the filter would be the bug. Stated at length
    // so the next reader can see it was considered rather than forgotten.
    let scan_started = std::time::Instant::now();
    let rows = match sessions.list_sessions(SessionFilter::default()).await {
        Ok(rows) => rows,
        Err(e) => return classify_rescope(Err(e), bound),
    };
    let scanned = rows.len();

    let keys: Vec<SessionKey> = rows
        .iter()
        .filter_map(|meta| SessionKey::from_key_string(&meta.key))
        .filter(|key| {
            binding::conversation_of(key).is_some_and(|(channel, peer_kind, peer_id)| {
                (channel.as_str(), peer_kind, peer_id.as_str()) == target
            })
        })
        .collect();

    // The cost of the scan, at whatever size this install actually is. The
    // approval for enumerating rests on "bind is a rare operator action", and
    // this is the line that lets an operator check that reasoning against
    // their own data rather than taking it on trust.
    tracing::debug!(
        scanned,
        matched = keys.len(),
        elapsed_ms = scan_started.elapsed().as_millis(),
        channel_id = %bound.channel_id,
        peer_id = %bound.peer_id,
        "projects.channel.bind: scanned the session store for rows belonging to this conversation"
    );

    let mut moved_by = Vec::new();
    for key in &keys {
        match sessions.rescope_attribution(key, &bound.project_id).await {
            Ok(true) => moved_by.push(key.agent_id().to_string()),
            // The row was listed a moment ago and is gone now — a concurrent
            // delete. Not an error, and not something moved.
            Ok(false) => {}
            Err(e) => return classify_rescope(Err(e), bound),
        }
    }
    classify_rescope(Ok(moved_by), bound)
}

/// Turn the scan's aggregate result into the three values the receipt carries
/// — and log the two things the enum cannot say.
///
/// Split from the `await`s above so the mapping is testable without a
/// thirty-method stub store: neither shipped backend can be made to return
/// `Err` on demand, and a hand-written stub that could would be a second
/// derivation of the thing under test. This function IS the production
/// mapping, called with the production arguments.
///
/// `Err` must not fold into "nothing moved". `RescopeOutcome::NothingToMove`
/// tells every client no row was found, which would be a confident factual
/// claim about a conversation whose store just failed. The bind itself has
/// already committed by the time this runs, so an error must not fail the RPC
/// — but a swallowed error with no log is how "something will heal it later"
/// becomes "nobody ever knew", hence the `warn!`.
///
/// An `Err` after some rows already moved is still [`RescopeOutcome::Unknown`],
/// not `Moved`: `Moved` asserts the conversation's transcript is in the room,
/// and with one row unmoved that assertion is false for part of it. `Unknown`
/// claims only that the outcome is unobserved, which is exactly the case.
///
/// 把扫描的聚合结果映射成收据的三个值，并把枚举说不出的两件事写进日志。
fn classify_rescope(
    result: Result<Vec<String>, crate::gateway::session_store::error::SessionStoreError>,
    bound: &ChannelBinding,
) -> RescopeOutcome {
    let moved_by = match result {
        Ok(moved_by) => moved_by,
        Err(e) => {
            tracing::warn!(
                error = %e,
                channel_id = %bound.channel_id,
                peer_id = %bound.peer_id,
                project_id = %bound.project_id,
                "projects.channel.bind: binding recorded, but the session store could not \
                 report whether an existing transcript moved"
            );
            return RescopeOutcome::Unknown;
        }
    };
    if moved_by.is_empty() {
        return RescopeOutcome::NothingToMove;
    }
    // More than one agent has served this conversation over its lifetime, so
    // the bind moved more than one row. That is an operator-visible oddity and
    // `Moved` — a three-valued enum — structurally cannot say it. This is not
    // a fourth variant; it is the log carrying the half the enum does not.
    if moved_by.len() > 1 {
        tracing::warn!(
            agent_ids = %moved_by.join(", "),
            moved = moved_by.len(),
            channel_id = %bound.channel_id,
            peer_id = %bound.peer_id,
            project_id = %bound.project_id,
            "projects.channel.bind: more than one agent has served this conversation; \
             every one of their session rows moved into the room"
        );
    }
    RescopeOutcome::Moved
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
    // carries; nothing on this path converts. (`binding::from_wire` exists for
    // the ROUTING enum, which this handler no longer needs — see Ruling AM.)
    let bound = match store.bind_conversation(
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

    // The STORED binding, not `params`: its `channel_id` / `peer_id` are
    // already normalized the way a live `SessionKey` normalizes, which is what
    // the scan below compares against.
    let rescoped = rescope_existing_transcript(sessions.as_ref(), &bound).await;

    if let Some(log) = crate::security::audit::global() {
        log.log(crate::security::audit::AuditEntry::authority_change(
            actor,
            // The NORMALIZED components, not the operator's raw spelling: this
            // record has to name the key that actually governs routing from
            // now on. The raw spelling is preserved in the binding's `label`.
            format!(
                "projects.channel.bind: {}:{} → {} (rescoped_session={rescoped})",
                bound.channel_id, bound.peer_id, project.id
            ),
        ));
    }
    crate::projects::events::publish_changed(&event_bus, &project.id, ChangeKind::Updated, None);

    JsonRpcResponse::success(
        request.id,
        json!(ChannelBindResult {
            binding: row(bound),
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

    /// A stored binding, for the classifier's `&ChannelBinding` argument.
    fn a_binding() -> ChannelBinding {
        ChannelBinding {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: BindingPeerKind::Group,
            peer_id: "c1".into(),
            bound_by: Some("u-alice".into()),
            bound_at: 0,
            label: None,
        }
    }

    /// Ruling AG, at the one place it is decided. `Err` must be its own answer:
    /// folding it into "nothing moved" makes every client report that no row
    /// was found for a conversation whose store just failed.
    ///
    /// This calls the production mapping directly. Neither shipped backend can
    /// be made to return `Err` on demand, and a stub that could would be a
    /// second derivation of the thing under test.
    #[test]
    fn a_store_error_is_reported_as_unknown_and_not_as_nothing_to_move() {
        let bound = a_binding();
        let moved = classify_rescope(Ok(vec!["main".to_string()]), &bound);
        let nothing = classify_rescope(Ok(Vec::new()), &bound);
        let failed = classify_rescope(Err(SessionStoreError::Unsupported), &bound);

        assert_eq!(moved, RescopeOutcome::Moved);
        assert_eq!(nothing, RescopeOutcome::NothingToMove);
        assert_eq!(failed, RescopeOutcome::Unknown);
        assert_ne!(
            failed, nothing,
            "a store that errored has not found nothing to move — a client \
             rendering the two identically asserts a result it never saw"
        );
    }

    /// `Moved` means "at least one row moved" (Ruling AM), so the enum is
    /// unchanged by the switch to enumeration and Tasks 10/11 need no further
    /// patching. Two moved rows are still `Moved`; the fact that there were
    /// two is carried by the `warn!`, which the enum structurally cannot say.
    #[test]
    fn moving_more_than_one_row_is_still_moved() {
        let bound = a_binding();
        assert_eq!(
            classify_rescope(Ok(vec!["main".into(), "coder".into()]), &bound),
            RescopeOutcome::Moved,
            "a three-valued receipt does not gain a fourth value for the \
             multi-agent case — the log carries that half"
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
        // `zz-standup`, not the `C1` the other tests use: see the assertion at
        // the end of this test for why the probed peer id must contain
        // non-hex letters.
        store
            .bind_conversation(
                &project.id,
                "telegram",
                BindingPeerKind::Group,
                "zz-standup",
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
        // ⚠️ The peer id probed for here MUST be one that cannot occur inside
        // a project id. Project ids are `p-<32 hex chars>`
        // (`store::mint_id`), so the first version of this assertion looked
        // for `"c1"` — the peer id the rest of the file uses — and fired on
        // roughly one run in nine when the fixture's own random id happened to
        // contain those two hex digits. It passed twice before failing on
        // `p-fa211571c4d24960bdcbe9c108d2f81e`.
        //
        // The lesson is not "that was unlucky": a negative assertion has to be
        // over an alphabet the haystack cannot produce, or it is testing the
        // random number generator. `zz-standup` and `telegram` both contain
        // non-hex letters and therefore cannot appear in a project id.
        assert!(
            !refused_msg.contains("telegram") && !refused_msg.contains("zz-standup"),
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

    /// The defect option A would have shipped, and the reason Ruling AM chose
    /// enumeration: the conversation's row lives under whichever agent the
    /// route resolved, and on a deployment that ran `channels.set_agent` that
    /// is not `"main"`. A handler that built one key named `"main"` finds
    /// nothing here, reports `NothingToMove`, and leaves the row
    /// `personal:<first speaker>` — the split this whole verb exists to close.
    #[tokio::test]
    async fn a_row_created_under_a_non_default_agent_is_still_moved() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        // NOT "main": this channel is pointed at another agent.
        let key = SessionKey::group("coder", "telegram", PeerKind::Group, "C1");
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
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::Moved,
            "the binding does not carry an agent id, so the search must not \
             assume one — this row is under `coder`"
        );
        assert_eq!(
            sessions
                .get_metadata(&key)
                .await
                .unwrap()
                .unwrap()
                .scope_id
                .as_deref(),
            Some(format!("project:{}", project.id).as_str())
        );
    }

    /// The case a single addressed key cannot express AT ALL: two agents have
    /// served this group over its lifetime, so there are two rows. Both must
    /// move — leaving one behind would keep that half of the conversation
    /// invisible to the roster.
    #[tokio::test]
    async fn every_agents_row_for_the_conversation_moves_not_just_one() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        let main_key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        let coder_key = SessionKey::group("coder", "telegram", PeerKind::Group, "C1");
        // A DIFFERENT conversation on the same channel, and the same peer id on
        // a different channel: neither may be swept up by the scan.
        let other_peer = SessionKey::group("main", "telegram", PeerKind::Group, "C2");
        let other_channel = SessionKey::group("main", "slack", PeerKind::Group, "C1");
        for k in [&main_key, &coder_key, &other_peer, &other_channel] {
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                sessions.get_or_create(k),
            )
            .await
            .unwrap();
        }

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

        let scope_of = |k: &SessionKey| {
            let sessions = sessions.clone();
            let k = k.clone();
            async move {
                sessions
                    .get_metadata(&k)
                    .await
                    .unwrap()
                    .unwrap()
                    .scope_id
                    .clone()
            }
        };
        let room_scope = Some(format!("project:{}", project.id));
        assert_eq!(scope_of(&main_key).await, room_scope, "main's row moved");
        assert_eq!(scope_of(&coder_key).await, room_scope, "coder's row moved");
        assert_eq!(
            scope_of(&other_peer).await,
            Some("personal:u-alice".to_string()),
            "a different peer id in the same channel is a different conversation"
        );
        assert_eq!(
            scope_of(&other_channel).await,
            Some("personal:u-alice".to_string()),
            "the same peer id on another channel is a different conversation"
        );
    }

    /// Ruling AM constraint 1, pinned so the reflex cannot be added back
    /// silently.
    ///
    /// The scan runs `SessionFilter::default()` — `owner_visible_to: None` —
    /// because the rows it is looking for BELONG TO OTHER PEOPLE.
    /// `personal:<first speaker>` is exactly the class being moved, and the
    /// first speaker is usually not the operator doing the bind. Adding
    /// `visible_owner_filter()` there reads like closing a hole; it would
    /// instead drop precisely the rows the bind exists to relocate, while the
    /// receipt still said it succeeded.
    ///
    /// Here `u-bob` spoke first and `u-alice` binds. Under a caller-scoped
    /// filter the row is invisible to alice and this test goes RED with
    /// `NothingToMove`.
    #[tokio::test]
    async fn a_row_owned_by_another_user_is_still_moved() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-bob")),
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
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::Moved,
            "the scan must not be narrowed to the caller: the row it is looking \
             for is owned by whoever spoke first, not by the operator binding"
        );
        let meta = sessions.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(
            meta.scope_id.as_deref(),
            Some(format!("project:{}", project.id).as_str())
        );
        assert_eq!(
            meta.owner_user_id.as_deref(),
            Some("u-bob"),
            "only the scope moves — the byline still names whoever spoke first"
        );
    }

    /// A key shape that is not a conversation at all must never be swept up.
    /// `conversation_of` returns `None` for a DM, and a DM has exactly one
    /// human on the far side: moving it into a room would put a shared
    /// partition behind a private conversation.
    #[tokio::test]
    async fn a_dm_on_the_same_channel_is_never_swept_into_the_room() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        let dm = SessionKey::dm(
            "main",
            "telegram",
            "C1",
            crate::routing::session_key::DmScope::PerPeer,
        );
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&dm),
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
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::NothingToMove,
            "a DM sharing the channel and peer id is not the bound conversation"
        );
        assert_eq!(
            sessions
                .get_metadata(&dm)
                .await
                .unwrap()
                .unwrap()
                .scope_id
                .as_deref(),
            Some("personal:u-alice"),
            "the DM stays where it was"
        );
    }

    /// Ruling AM constraint 4: the scan's cost, measured rather than reasoned
    /// about.
    ///
    /// Enumerating was approved on the judgement that `bind` is a rare
    /// operator action. That judgement is reasoning, not measurement, so this
    /// seeds a store with many unrelated conversations and checks the needle is
    /// still found — and prints the wall time for the report.
    ///
    /// **Timing is printed, never asserted.** A wall-clock threshold in a test
    /// suite that runs 17k tests in parallel on shared hardware is a flake
    /// generator, and a flaky guard is worse than none: it teaches people to
    /// re-run until green. The assertion here is on CORRECTNESS at scale — the
    /// one row that should move did, and the 200 that should not did not.
    ///
    /// The shape of the cost, for anyone extrapolating: one `list_sessions`
    /// (the file backend reads every session's metadata) plus O(n) key parses.
    /// It grows linearly with the number of sessions on the install, and it is
    /// paid once per `bind`.
    #[tokio::test]
    async fn the_scan_still_finds_the_conversation_among_many_unrelated_sessions() {
        const NOISE: usize = 200;
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        for i in 0..NOISE {
            let noise = SessionKey::group("main", "telegram", PeerKind::Group, format!("noise{i}"));
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                sessions.get_or_create(&noise),
            )
            .await
            .unwrap();
        }
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        let started = std::time::Instant::now();
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
        let elapsed = started.elapsed();
        eprintln!(
            "[scan cost] {} session rows in the store, bind took {} ms",
            NOISE + 1,
            elapsed.as_millis()
        );

        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);
        assert_eq!(
            sessions
                .get_metadata(&key)
                .await
                .unwrap()
                .unwrap()
                .scope_id
                .as_deref(),
            Some(format!("project:{}", project.id).as_str()),
            "the needle moved"
        );
        let stray = SessionKey::group("main", "telegram", PeerKind::Group, "noise0");
        assert_eq!(
            sessions
                .get_metadata(&stray)
                .await
                .unwrap()
                .unwrap()
                .scope_id
                .as_deref(),
            Some("personal:u-alice"),
            "and nothing else did — a scan that matched loosely would sweep the \
             whole channel into the room"
        );
    }
}
