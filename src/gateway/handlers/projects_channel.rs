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
use crate::gateway::event_visibility::EventVisibilityIndex;
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
///
/// # What this scan can silently miss, and therefore what `NothingToMove` costs
///
/// The receipt is only as honest as `list_sessions` is complete, so the ways it
/// can come back short are disclosed here rather than left to be rediscovered.
/// None of these is fixed by this handler — two live in the backends and one is
/// inherent — but a reader owes themselves the list before trusting a
/// `NothingToMove`:
///
/// - **SQLite backend (the shipped default, `[general] session_store =
///   "sqlite"`).** `session_manager::ops::query::list_sessions` collects with
///   `rows.filter_map(|r| r.ok())` — a row whose column mapping fails is
///   dropped **in complete silence**. One damaged `sessions` row for the bound
///   conversation therefore yields `NothingToMove`, which is Ruling AG's defect
///   one layer down: an `Err` folded into an absence, and a receipt then
///   asserting the absence. Not fixed here deliberately — that `filter_map` is
///   pre-existing, has other callers, and re-classifying it is its own task.
/// - **File backend.** Same class, but it is *loud*: an unparseable
///   `metadata.json` is skipped with a named `warn!`. An unreadable one (IO
///   error rather than parse error) is still silent.
/// - **The listing is a snapshot.** A turn whose routing resolved
///   `room_claiming` before `bind_conversation` committed, but whose session row
///   is created after this listing, is stamped `personal:<speaker>` and never
///   seen. `stamp_attribution` is create-only and `backfill_attribution` heals
///   only NULL rows, so that row stays personal. The window is milliseconds and
///   the remedy is free: **re-running `bind` on the same room is a documented
///   no-op that re-runs this scan.** Said out loud so the next person to meet it
///   looks at the clock rather than at the matching logic.
///
/// 这次扫描可能静默漏掉什么——写出来，因为 `NothingToMove` 的诚实程度取决于
/// `list_sessions` 的完整程度：SQLite（出厂默认）静默丢坏行、file 后端对解析失败
/// 出声但对读失败不出声、以及快照与并发回合的赛跑（重跑 bind 即可再扫一遍）。
async fn rescope_existing_transcript(
    sessions: &dyn SessionStore,
    bound: &ChannelBinding,
    // Invalidated per moved key, right after that key's write commits (see
    // the loop below) — never before it, and never batched to the end of the
    // scan. A concurrent `session_admits` landing in the gap between a
    // premature forget and the write actually committing would re-cache the
    // PRE-bind pair with nothing left to invalidate it again, so the order
    // is load-bearing, not a convenience.
    event_visibility: &EventVisibilityIndex,
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

    // Every field written out, and deliberately NOT `..Default::default()`.
    //
    // Each of the four is a way to silently miss rows, and a missed row is not
    // a visible failure — it is a `NothingToMove` receipt about a conversation
    // that has a transcript. Spelling them all out also means a future field on
    // `SessionFilter` is a COMPILE ERROR here, which forces that author to
    // answer "is this a fifth way to miss rows?" instead of inheriting a
    // default that quietly says no.
    let filter = SessionFilter {
        // The axis the previous implementation guessed. A binding carries no
        // agent id, so constraining this to any single one reintroduces exactly
        // the defect Ruling AM removed.
        agent_id: None,
        // No truncation. A truncated scan that then reports `NothingToMove` is
        // a no-op reporting success.
        //
        // Verified rather than assumed, in both shipped backends: the file
        // backend's `list_sessions` is an unbounded `read_dir` loop with no
        // cap, and the sqlite backend's SQL is `SELECT ... ORDER BY
        // last_active_at DESC` with no `LIMIT` clause, truncating in memory
        // only `if let Some(limit)`. `None` really is unbounded on both.
        limit: None,
        // A group nobody has spoken in for months is not less bound — it is the
        // case most in need of moving, because its transcript is the one people
        // have forgotten is stranded under a personal scope.
        active_minutes: None,
        // ⚠️ This `None` is a FUNCTIONAL REQUIREMENT, not a missing gate.
        //
        // The rows being searched for may belong to ANYBODY: `personal:<first
        // speaker>` is precisely the class this verb exists to move, and the
        // first speaker is usually not the operator running the bind. Adding
        // `visible_owner_filter()` here — the reflex, and correct almost
        // everywhere else in this crate — would silently drop exactly the rows
        // the bind is supposed to relocate, while the receipt still said it
        // succeeded.
        //
        // The admission decision was already made upstream: `bind` is
        // admin-gated in `method_admin::ADMIN_METHODS`, and `gate_project` has
        // already refused a room the caller cannot see. An operator entitled to
        // bind the room is entitled to move its conversation's rows.
        //
        // This is the mirror image of the repo's `..Default::default()`
        // criterion, where leaving this same field `None` was the DEFECT
        // (`session_list` showing every owner's rows). Same field, opposite
        // direction: there the filter was the point, here the filter would be
        // the bug. `a_row_owned_by_another_user_is_still_moved` reddens if
        // anyone adds it.
        owner_visible_to: None,
    };
    let scan_started = std::time::Instant::now();
    let rows = match sessions.list_sessions(filter).await {
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
            // The write has committed at this point — `rescope_attribution`
            // returning is the commit — so invalidating THIS key's cached
            // `(owner_user_id, scope_id)` pair here can only ever throw away
            // a stale value, never a fresh one. Per key, not once after the
            // loop: `keys` can name more than one agent's row for the same
            // conversation, and every one of them just changed scope.
            Ok(true) => {
                event_visibility.forget_session(&key.to_key_string()).await;
                moved_by.push(key.agent_id().to_string());
            }
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
/// Split from the `await`s above so the mapping is testable in isolation
/// without a thirty-method stub store — a stub that could return `Err` on
/// demand would be a second derivation of the thing under test. This function
/// IS the production mapping, called with the production arguments.
///
/// ⚠️ It used to say here that neither shipped backend can be made to return
/// `Err` at all. **That stopped being true when the scan was added**, and the
/// sentence outlived its truth by one commit: `list_sessions` is a second
/// `Err` source and a real `FileSessionStore` produces one if its base
/// directory is removed. `a_store_that_cannot_be_listed_reports_unknown_and_still_records_the_binding`
/// exercises exactly that, end to end. Corrected rather than deleted because a
/// written "this cannot be done" does not just describe the code wrongly — it
/// talks the next reader out of the fix.
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
    event_visibility: Arc<EventVisibilityIndex>,
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
    // carries. Nothing on this path converts to the routing `PeerKind` at all —
    // the enumeration compares wire kinds, so no inverse of `binding::to_wire`
    // is needed anywhere (Ruling AM; the inverse was deleted for having no
    // consumer).
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
    let rescoped =
        rescope_existing_transcript(sessions.as_ref(), &bound, event_visibility.as_ref()).await;

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
        )).await;
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
/// # It deliberately does NOT check room visibility, unlike `bind`
///
/// [`handle_bind`] runs [`gate_project`]; this does not, and cannot: it is
/// addressed by CONVERSATION (`channel_id`, `peer_kind`, `peer_id`) and carries
/// no project id for a gate to gate on. The only way to add one would be to
/// resolve the owning room and refuse on it. Read cold, the asymmetry looks
/// like an oversight, and "symmetrizing" it is a one-line change that would
/// pass review — so the three reasons live here, where the next reader is
/// standing, and not only in a report:
///
/// 1. **It runs in the safe direction.** `bind` creates an outward exposure;
///    `unbind` removes one. "Anyone who may call this can always stop it" is
///    the failure mode you want.
/// 2. **Gating it would make a stale binding un-detachable** — a door with no
///    handle is not a gate, it is a wall. This is reachable, not hypothetical:
///    `projects.remove` deletes the roster, and the roster IS the visibility
///    predicate, so a conversation bound to a room no principal can see is a
///    state the system can reach. Gated, that binding could never be released.
/// 3. **The authority exercised is over the CHANNEL CONVERSATION, not the
///    room** — "this Telegram group stops being a room conversation" is a
///    channel-level decision, and it is audit-logged with the room it was
///    taken from.
///
/// Access control for this verb is the admin gate in
/// [`method_admin::ADMIN_METHODS`], pinned separately. Nothing leaks either:
/// the response is `{unbound: bool}`, and that bool answers an operator-only
/// question anyway.
///
/// Pinned by `unbind_deliberately_succeeds_for_a_room_the_caller_cannot_see`.
/// If that goes red because somebody added a visibility gate, it is a product
/// decision to re-open with a human, not a test to update.
///
/// 解绑刻意不做房间可见性检查（`bind` 做）。三条理由见上：方向安全、
/// 加闸会造出解不掉的陈旧绑定（没有门把手的门是墙）、权限主体是频道会话而非房间。
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
/// not exist. Every surface must therefore print
/// [`aleph_protocol::projects::UNBIND_KEEPS_TRANSCRIPT_NOTICE`] after a
/// successful unbind.
///
/// The sentence is **not repeated here on purpose.** It is unconditionally
/// true, so it is client copy rather than wire state (Ruling AI) — but copy
/// that must be byte-identical on three surfaces needs one author, and quoting
/// it in this doc would make this the second. See that constant for the whole
/// argument.
///
/// 解绑不会把已有记录搬回去：这是有意的，因为没有一个正确的目的地可搬。
/// 每个客户端面都必须打印 `UNBIND_KEEPS_TRANSCRIPT_NOTICE`——**这里刻意不复述那句话**，
/// 复述就是给它第二个作者。
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
            )).await;
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
    // The whole row vocabulary the `ReadDuringRescope` decorator has to name
    // in its delegating signatures, kept behind one alias so the delegation is
    // readable as delegation.
    use crate::gateway::session_store::types as store_types;
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

    fn visibility() -> Arc<EventVisibilityIndex> {
        Arc::new(EventVisibilityIndex::new())
    }

    /// `event_admits`'s entry point for a plain `BySessionKey` frame — no
    /// `OrAdmin` shortcut, so this is the honest probe for whether a caller's
    /// cached `(owner_user_id, scope_id)` verdict for `key` is stale.
    /// `stream.session_updated` is one of the topics
    /// `session_identity_of` classifies this way from the frame's own
    /// `session_key` field.
    async fn admits(
        index: &EventVisibilityIndex,
        key: &SessionKey,
        caller: &str,
        caller_is_admin: bool,
        store: &Arc<dyn SessionStore>,
    ) -> bool {
        index
            .event_admits(
                "stream.session_updated",
                Some(&json!({ "session_key": key.to_key_string() })),
                Some(caller),
                caller_is_admin,
                store,
                None,
            )
            .await
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
    /// This calls the production mapping directly, which is what makes it a
    /// unit test of the mapping rather than of the plumbing. The `Err` arm is
    /// ALSO covered end to end by
    /// `a_store_that_cannot_be_listed_reports_unknown_and_still_records_the_binding`
    /// — the two are complementary, and neither replaces the other: this one
    /// pins which value each outcome maps to, that one pins that the value
    /// reaches the wire and that the bind is not rolled back on the way.
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
                    visibility(),
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

    /// The wire T11 exists for, asserted as EFFECT rather than as a call: a
    /// room-mate who was DENIED this conversation's live frames before the
    /// bind — and whose denial the ownership cache is now holding — is
    /// admitted right after it, with nobody touching the cache by hand.
    ///
    /// The pre-bind `admits` call is not a courtesy, it is the setup: it is
    /// what warms `EventVisibilityIndex`'s `(owner_user_id, scope_id)` cache
    /// with the pre-bind pair, and a real deployment is always in that state
    /// (`project_for` runs `session_admits` for every running key on the
    /// Global `running_set_changed` frame, so any group that ran while any
    /// socket was open is warm). That cache has no TTL and evicts only by
    /// FIFO at `MAX_CACHED_SESSION_OWNERS`, so without the invalidation the
    /// final assertion below stays false for the process lifetime.
    #[tokio::test]
    async fn a_room_mate_stops_being_denied_the_conversations_frames_after_the_bind() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();
        let index = visibility();

        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        assert!(
            !admits(&index, &key, "u-bob", false, &sessions).await,
            "before the bind the row is personal:u-alice, so u-bob is denied \
             — and that verdict's two inputs are now in the cache"
        );

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    index.clone(),
                ),
            )
            .await;
        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);

        assert!(
            admits(&index, &key, "u-bob", false, &sessions).await,
            "the row now carries the room scope and u-bob is on that roster, \
             so the delivery plane must admit him. A stale cached pair here is \
             the whole defect: the binding looks correct on every surface \
             while every other member's live frames stay silently denied"
        );
    }

    /// Per KEY, not per outcome. One conversation can have been served by more
    /// than one agent over its lifetime — that is exactly why
    /// `rescope_existing_transcript` enumerates instead of addressing a single
    /// key (Ruling AM) — and every row it moves has just changed scope, so
    /// every one of them has a cached pair to drop.
    ///
    /// Forgetting only the first key (or only once, after the loop) leaves the
    /// other agent's frames denied while the receipt says `Moved`.
    #[tokio::test]
    async fn every_moved_agent_key_is_invalidated_not_only_the_first() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();
        let index = visibility();

        let main = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        let coder = SessionKey::group("coder", "telegram", PeerKind::Group, "C1");
        for key in [&main, &coder] {
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                sessions.get_or_create(key),
            )
            .await
            .unwrap();
            assert!(
                !admits(&index, key, "u-bob", false, &sessions).await,
                "both rows are personal:u-alice pre-bind, and both verdicts \
                 are now cached"
            );
        }

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    index.clone(),
                ),
            )
            .await;
        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);

        for key in [&main, &coder] {
            assert!(
                admits(&index, key, "u-bob", false, &sessions).await,
                "{} was moved by this bind, so its cached pair must have been \
                 dropped too — the loop invalidates per key, and the scan is \
                 unordered, so 'the first one' is not a key this code may \
                 privilege",
                key.to_key_string()
            );
        }
    }

    /// The operator who RAN the bind is not exempt: `stream.session_updated`
    /// is classified `BySessionKey`, which has no admin short-circuit (only
    /// `BySessionKeyOrAdmin` does), so an operator's connection warms and
    /// reads the same cache every member does.
    ///
    /// The first speaker here is `u-bob`, not the operator, so the operator's
    /// pre-bind verdict is a DENIAL — the room owner cannot see a member's
    /// personal row — and it is that denial the cache holds. Post-bind she is
    /// admitted through the roster like anyone else. Without the
    /// invalidation, running the bind blinds the person who ran it.
    #[tokio::test]
    async fn the_operator_who_ran_the_bind_sees_the_post_bind_answer_too() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();
        let index = visibility();

        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        assert!(
            !admits(&index, &key, "u-alice", true, &sessions).await,
            "an operator flag does not shortcut a BySessionKey frame, so even \
             the room owner is denied a member's personal row — and that is \
             the verdict now cached for her connection"
        );

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    index.clone(),
                ),
            )
            .await;
        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);

        assert!(
            admits(&index, &key, "u-alice", true, &sessions).await,
            "the bind moved the row into her own room; if her cached denial \
             survives it, the operator's reward for binding is silence"
        );
    }

    /// A `SessionStore` decorator whose only seam is `rescope_attribution`:
    /// just BEFORE the inner write commits, it runs the delivery plane's own
    /// read for `reader`, which re-caches whatever `(owner_user_id, scope_id)`
    /// pair the row holds at THAT instant — the pre-bind one.
    ///
    /// That is the concurrent `session_admits` the forget's placement is
    /// defending against, run inline so the ordering is observable
    /// deterministically instead of raced against a second task. Every other
    /// method delegates, so `handle_bind` still runs against a real backend.
    struct ReadDuringRescope {
        inner: Arc<dyn SessionStore>,
        index: Arc<EventVisibilityIndex>,
        reader: String,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SessionStore for ReadDuringRescope {
        async fn rescope_attribution(
            &self,
            key: &SessionKey,
            project_id: &str,
        ) -> Result<bool, SessionStoreError> {
            // The read lands here: the row still holds the pre-bind pair, so
            // this re-caches it.
            let _ = admits(&self.index, key, &self.reader, false, &self.inner).await;
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.rescope_attribution(key, project_id).await
        }

        async fn get_or_create(
            &self,
            key: &SessionKey,
        ) -> Result<store_types::SessionMetadata, SessionStoreError> {
            self.inner.get_or_create(key).await
        }
        async fn get_metadata(
            &self,
            key: &SessionKey,
        ) -> Result<Option<store_types::SessionMetadata>, SessionStoreError> {
            self.inner.get_metadata(key).await
        }
        async fn list_sessions(
            &self,
            filter: SessionFilter,
        ) -> Result<Vec<store_types::SessionMetadata>, SessionStoreError> {
            self.inner.list_sessions(filter).await
        }
        async fn delete_session(
            &self,
            key: &SessionKey,
        ) -> Result<store_types::DeleteResult, SessionStoreError> {
            self.inner.delete_session(key).await
        }
        async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
            self.inner.reset_session(key).await
        }
        async fn append_message(
            &self,
            key: &SessionKey,
            msg: store_types::MessageRecord,
        ) -> Result<(), SessionStoreError> {
            self.inner.append_message(key, msg).await
        }
        async fn get_history(
            &self,
            key: &SessionKey,
            limit: Option<usize>,
        ) -> Result<Vec<store_types::MessageRecord>, SessionStoreError> {
            self.inner.get_history(key, limit).await
        }
        async fn search_messages(
            &self,
            query: &str,
            max_results: usize,
        ) -> Result<Vec<store_types::SearchHit>, SessionStoreError> {
            self.inner.search_messages(query, max_results).await
        }
        async fn list_checkpoints(
            &self,
            key: &SessionKey,
        ) -> Result<Vec<store_types::CheckpointSummary>, SessionStoreError> {
            self.inner.list_checkpoints(key).await
        }
        async fn branch_from_checkpoint(
            &self,
            key: &SessionKey,
            checkpoint_id: &str,
            new_key: &SessionKey,
        ) -> Result<store_types::SessionMetadata, SessionStoreError> {
            self.inner
                .branch_from_checkpoint(key, checkpoint_id, new_key)
                .await
        }
        async fn restore_checkpoint(
            &self,
            key: &SessionKey,
            checkpoint_id: &str,
        ) -> Result<store_types::SessionMetadata, SessionStoreError> {
            self.inner.restore_checkpoint(key, checkpoint_id).await
        }
        async fn close_session(
            &self,
            key: &SessionKey,
            topic: Option<&str>,
        ) -> Result<(), SessionStoreError> {
            self.inner.close_session(key, topic).await
        }
        async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
            self.inner.set_topic(key, topic).await
        }
        async fn set_state(
            &self,
            key: &SessionKey,
            state: crate::gateway::session_manager::SessionState,
        ) -> Result<(), SessionStoreError> {
            self.inner.set_state(key, state).await
        }
        async fn get_state(
            &self,
            key: &SessionKey,
        ) -> Result<crate::gateway::session_manager::SessionState, SessionStoreError> {
            self.inner.get_state(key).await
        }
        async fn get_identity_context(
            &self,
            session_key: &str,
            source_channel: &str,
        ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
            self.inner
                .get_identity_context(session_key, source_channel)
                .await
        }
        async fn get_current_epoch(
            &self,
            base_key_pattern: &str,
        ) -> Result<u32, SessionStoreError> {
            self.inner.get_current_epoch(base_key_pattern).await
        }
        async fn get_session_topic(
            &self,
            key: &SessionKey,
        ) -> Result<Option<String>, SessionStoreError> {
            self.inner.get_session_topic(key).await
        }
        async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
            self.inner.cleanup_expired().await
        }
        async fn patch_session(
            &self,
            key: &SessionKey,
            patch: &store_types::SessionPatch,
        ) -> Result<bool, SessionStoreError> {
            self.inner.patch_session(key, patch).await
        }
        async fn update_session_usage(
            &self,
            key: &SessionKey,
            input_tokens: i64,
            output_tokens: i64,
            cost_usd: f64,
            model: Option<&str>,
            model_provider: Option<&str>,
        ) -> Result<(), SessionStoreError> {
            self.inner
                .update_session_usage(
                    key,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    model,
                    model_provider,
                )
                .await
        }
        async fn get_session_preview(
            &self,
            key: &SessionKey,
            message_limit: usize,
        ) -> Result<store_types::SessionPreview, SessionStoreError> {
            self.inner.get_session_preview(key, message_limit).await
        }
        async fn count_by_state(
            &self,
            state: crate::gateway::session_manager::SessionState,
        ) -> Result<usize, SessionStoreError> {
            self.inner.count_by_state(state).await
        }
        async fn list_by_state(
            &self,
            state: crate::gateway::session_manager::SessionState,
        ) -> Result<Vec<store_types::SessionMetadata>, SessionStoreError> {
            self.inner.list_by_state(state).await
        }
        async fn set_error(
            &self,
            key: &SessionKey,
            error_msg: Option<&str>,
        ) -> Result<(), SessionStoreError> {
            self.inner.set_error(key, error_msg).await
        }
        async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
            self.inner.stop(key).await
        }
        async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
            self.inner.set_idle(key).await
        }
    }

    /// Ordering: the forget must come AFTER the write commits.
    ///
    /// A forget issued before it has nothing left to protect — the read
    /// [`ReadDuringRescope`] lands a moment later re-caches the pre-bind pair,
    /// and nothing invalidates it a second time — so the room-mate stays
    /// denied even though the row moved. Moving the `forget_session` call
    /// above `rescope_attribution` reddens this test and leaves the plain
    /// effect test green, which is why the two are not one test.
    #[tokio::test]
    async fn a_read_landing_inside_the_rescope_write_cannot_leave_the_stale_pair_cached() {
        let (store, project, _guard) = room();
        let (inner, _dir) = sessions();
        let index = visibility();

        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            inner.get_or_create(&key),
        )
        .await
        .unwrap();

        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sessions: Arc<dyn SessionStore> = Arc::new(ReadDuringRescope {
            inner: inner.clone(),
            index: index.clone(),
            reader: "u-bob".to_string(),
            reads: reads.clone(),
        });

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    index.clone(),
                ),
            )
            .await;
        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(result.rescoped_session, RescopeOutcome::Moved);
        assert_eq!(
            reads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the probe must actually have run inside the write — a decorator \
             the bind path stopped calling would make the assertion below pass \
             for the wrong reason"
        );

        assert!(
            admits(&index, &key, "u-bob", false, &inner).await,
            "a read that landed while the write was in flight re-cached the \
             PRE-bind pair; only a forget that happens after the write can \
             throw it away"
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
                    visibility(),
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
        // `zz-standup` rather than the `C1` the other tests use. The probe that
        // needed a non-hex spelling is gone (see the end of this test), but the
        // room must still HAVE a binding for the refusal to be worth asserting:
        // a stranger being refused an unbound room proves less.
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
        // The no-oracle property is carried ENTIRELY by the `assert_eq!` above:
        // pinning the message to exactly `project not found: {id}` already
        // makes leaking `telegram` or the peer id impossible. A `contains`
        // probe underneath it could not fail, so it is documentation dressed
        // as an assertion, and it is gone.
        //
        // ⚠️ Keeping the lesson it cost, because it generalises past this file.
        // That probe originally looked for `"c1"` — the peer id the rest of the
        // file uses. Project ids are `p-<32 hex chars>` (`store::mint_id`), and
        // `c1` is two hex digits, so it fired on roughly one run in nine when
        // the fixture's own random id happened to contain them. It passed twice
        // before failing on `p-fa211571c4d24960bdcbe9c108d2f81e`.
        //
        // The lesson is not "that was unlucky": **a negative assertion has to be
        // over an alphabet the haystack cannot produce, or it is testing the
        // random number generator.** The second lesson arrived with the review:
        // the assertion that cost a cycle to repair was the one carrying no
        // weight. Repairing a guard is not the same as checking it guards
        // anything.
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
                    visibility(),
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
                    visibility(),
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
                    visibility(),
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
                    visibility(),
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
                    visibility(),
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
                    visibility(),
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
    /// The scan sets `owner_visible_to: None` — see the filter in
    /// `rescope_existing_transcript`, where all four fields are written out —
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
                    visibility(),
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
                    visibility(),
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
                    visibility(),
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

    /// Ruling AM 1b: both sides of the comparison must be normalized, and this
    /// is the test that says so.
    ///
    /// `binding::conversation_of` reports the components of a live
    /// `SessionKey`, which `SessionKey::group` already ran through
    /// `sanitize_component` — so they are lowercased and punctuation-folded.
    /// The operator's `params.channel_id` / `params.peer_id` are raw. Comparing
    /// those two directly is the original defect resurrected on a different
    /// axis: one letter of case and nothing matches, and the receipt says
    /// `NothingToMove` about a conversation with a full transcript.
    ///
    /// The handler avoids it by comparing against the STORED `ChannelBinding`,
    /// whose components `bind_conversation` normalized through
    /// `binding::normalize_component` — the same function, so the two sides
    /// cannot drift. This test is what keeps that true: the operator types
    /// `Telegram` / `C1`, the live key is `telegram` / `c1`, and the row must
    /// still move.
    #[tokio::test]
    async fn an_operator_spelling_that_differs_only_in_case_still_finds_the_conversation() {
        let (store, project, _guard) = room();
        let (sessions, _dir) = sessions();

        // The live key, as an inbound message would mint it: normalized.
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "c1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            sessions.get_or_create(&key),
        )
        .await
        .unwrap();

        // The operator's spelling, as typed: capitalised, and not what any
        // session key or binding row contains.
        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc(
                        "projects.channel.bind",
                        json!({
                            "project_id": project.id,
                            "channel_id": "TeLeGrAm",
                            "peer_kind": "group",
                            "peer_id": "C1",
                        }),
                    ),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    visibility(),
                ),
            )
            .await;

        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::Moved,
            "the operator's raw spelling must be normalized before it is compared \
             against what `conversation_of` reports, or one letter of case means \
             zero rows match and the receipt reports NothingToMove about a \
             conversation that has a transcript"
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

    /// The `bind` / `unbind` visibility asymmetry is a RULING, not an oversight
    /// — and this test exists because a ruling that lives only in prose does not
    /// stop the next sincere fixer.
    ///
    /// `handle_bind` runs `gate_project`; `handle_unbind` does not, and cannot:
    /// it is addressed by CONVERSATION (`channel_id`, `peer_kind`, `peer_id`)
    /// and carries no project id for a gate to gate on. Read cold that looks
    /// like a hole, and closing it is a one-line change that would pass review.
    ///
    /// Three reasons it stays open, all of which have to fail together before
    /// this should be revisited:
    ///
    /// 1. The asymmetry runs in the SAFE direction. `bind` creates an outward
    ///    exposure; `unbind` removes one. "Anyone who may call this can always
    ///    stop it" is the failure mode you want.
    /// 2. Closing it would make a stale binding for a room the caller cannot see
    ///    permanently undetachable — **a door with no handle is not a gate, it
    ///    is a wall** — and would turn the refusal into an existence oracle in
    ///    the other direction.
    /// 3. The authority exercised is over the CHANNEL CONVERSATION, not the
    ///    room: "this Telegram group stops being a room conversation" is a
    ///    channel-level decision, and it is audit-logged with the room it was
    ///    taken from.
    ///
    /// Access control for this verb is the admin gate in
    /// `method_admin::ADMIN_METHODS`, pinned separately by
    /// `the_admin_gated_channel_binding_verbs_are_deliberately_absent`. This
    /// test is the other half: it pins that unbind does NOT additionally gate on
    /// room visibility, so re-adding that gate fails here by name.
    ///
    /// 解绑刻意不做房间可见性检查：这是裁决不是疏漏，三条理由见上。
    #[tokio::test]
    async fn unbind_deliberately_succeeds_for_a_room_the_caller_cannot_see() {
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

        // u-mallory is on no roster: `gate_project` refuses them this room, as
        // `a_non_member_cannot_list_a_rooms_bindings` proves for the sibling
        // verb. Unbind still succeeds, on purpose.
        let resp = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_unbind(
                    rpc(
                        "projects.channel.unbind",
                        json!({
                            "channel_id": "telegram",
                            "peer_kind": "group",
                            "peer_id": "C1",
                        }),
                    ),
                    store.clone(),
                    bus(),
                ),
            )
            .await;

        let result: ChannelUnbindResult = serde_json::from_value(
            resp.result
                .expect("unbind is not gated on room visibility — see this test's doc"),
        )
        .expect("unbind result");
        assert!(
            result.unbound,
            "unbind is deliberately NOT gated on room visibility. If this fails \
             because somebody added `gate_project` to `handle_unbind`, that is a \
             product ruling to re-open with a human, not a test to update: a \
             stale binding for a room nobody can see would become permanently \
             undetachable."
        );
        assert_eq!(
            store
                .project_for_conversation("telegram", BindingPeerKind::Group, "C1")
                .unwrap(),
            None,
            "and it really released the conversation"
        );
    }

    /// MEDIUM-2: the SHIPPED DEFAULT backend, exercised at handler level.
    ///
    /// `[general] session_store` defaults to `"sqlite"`
    /// (`config/types/general.rs`), and every other `handle_bind` test in this
    /// file builds a `FileSessionStore`. So until this test the scan had never
    /// run against the store most installs actually use — and the two backends
    /// are not interchangeable for this verb's purposes: they disagree about
    /// what happens to a row they cannot decode (file warns by name, sqlite's
    /// `filter_map(|r| r.ok())` drops it silently). See
    /// `rescope_existing_transcript`'s doc for what that costs a
    /// `NothingToMove`.
    ///
    /// This does not fix that asymmetry — it is pre-existing and has other
    /// callers. It makes sure the default path is executed at all, so a future
    /// change that works on one backend and not the other cannot pass unseen.
    ///
    /// 出厂默认是 sqlite，而其余 14 条 handler 测试全跑 file 后端；这一条让默认路径
    /// 至少被执行一次。
    #[tokio::test]
    async fn the_scan_works_on_the_shipped_default_sqlite_backend() {
        let (store, project, _guard) = room();
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions: Arc<dyn SessionStore> = Arc::new(
            crate::gateway::session_manager::SessionManager::new(
                crate::gateway::session_manager::SessionManagerConfig {
                    db_path: dir.path().join("sessions.db"),
                    ..Default::default()
                },
            )
            .expect("sqlite session store"),
        );

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
                    visibility(),
                ),
            )
            .await;

        let result: ChannelBindResult =
            serde_json::from_value(resp.result.expect("bind succeeds")).expect("bind result");
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::Moved,
            "the enumeration must find the row on the default backend too — \
             `list_sessions`, key parsing and `rescope_attribution` are all \
             separate implementations there"
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

    /// MEDIUM-3: the `Unknown` arm, end to end, on a real backend.
    ///
    /// The doc used to claim no shipped backend could be made to return `Err`.
    /// That was true in round 1, when `rescope_attribution` was the only `Err`
    /// source — and **round 2 falsified it** by adding the scan:
    /// `list_sessions` on a `FileSessionStore` whose base directory has been
    /// removed is `tokio::fs::read_dir` on a missing path, i.e. `Err`, with no
    /// stub anywhere.
    ///
    /// This pins the two properties the whole Ruling AG argument rests on, and
    /// which `a_store_error_is_reported_as_unknown_and_not_as_nothing_to_move`
    /// structurally cannot see because it calls the mapping directly:
    ///
    /// 1. `Unknown` **reaches the wire** — a client is told the outcome was not
    ///    observed, rather than being told no row was found.
    /// 2. The bind **is not rolled back** by the failure. The binding committed
    ///    before the scan ran, and a scan that cannot see the store must not
    ///    undo it; otherwise a transient IO error silently discards an
    ///    operator's decision.
    ///
    /// 这两条性质此前只活在散文里。
    #[tokio::test]
    async fn a_store_that_cannot_be_listed_reports_unknown_and_still_records_the_binding() {
        let (store, project, _guard) = room();
        let (sessions, dir) = sessions();

        // Make the store genuinely unlistable. No stub: this is the shipped
        // `FileSessionStore` meeting a missing base directory.
        std::fs::remove_dir_all(dir.path()).expect("remove the session store's base dir");

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_bind(
                    rpc("projects.channel.bind", bind_params(&project.id, "C1")),
                    store.clone(),
                    sessions.clone(),
                    bus(),
                    visibility(),
                ),
            )
            .await;

        let result: ChannelBindResult = serde_json::from_value(
            resp.result
                .expect("a scan failure must NOT fail the RPC — the bind already committed"),
        )
        .expect("bind result");
        assert_eq!(
            result.rescoped_session,
            RescopeOutcome::Unknown,
            "an unreadable store means the transcript's fate is unobserved. \
             Reporting NothingToMove here would be the receipt asserting an \
             absence it never saw — the exact defect Ruling AG removed."
        );
        assert_eq!(
            store
                .project_for_conversation("telegram", BindingPeerKind::Group, "C1")
                .unwrap()
                .as_deref(),
            Some(project.id.as_str()),
            "the binding committed before the scan ran and must survive its \
             failure — a transient IO error must not discard the operator's \
             decision"
        );
    }
}
