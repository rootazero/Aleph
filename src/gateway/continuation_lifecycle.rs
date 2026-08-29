//! Terminate a retired session's autonomous continuations (loop + goal) and
//! delete the `/btw` side session derived from it.
//!
//! Loops and goals are keyed by the FULL session string (epoch included) and
//! their continuation chains are self-sustaining — each tick's completion
//! re-enters the post-run hook under the OLD key. When a session is retired
//! by an epoch bump, every subsequent user message (including `/loop stop`
//! and `goal clear`) routes to the NEW epoch, where the tools honestly report
//! "no loop/goal in this session" — leaving an uncancellable background chain
//! posting to the channel for up to its full tick cap.
//!
//! This is the single seam every surface that retires a session must call
//! BEFORE the bump: the channel `/new` command
//! (`inbound_router::command_handler`), the Panel `sessions.new` RPC
//! (`handlers::session::db_handlers::create`), and the `sessions.delete` RPC
//! (`handlers::session::db_handlers::modify`) behind the Panel sidebar's
//! delete and the CLI's `session delete` — deletion retires the key just as
//! finally as an epoch bump, and the chains are keyed by the string, not by
//! the transcript's existence.
//!
//! The side session rides along here for the same reason: it is derived from
//! the retiring key (epoch included), so retiring that key is exactly the
//! moment it becomes unreachable, and nothing else in the tree ever deleted
//! one. Each of those three surfaces already holds a `SessionStore` handle, so
//! the handle is a parameter — see
//! [`terminate_session_continuations`] for why it is not fetched from a global.
//!
//! Mechanical state termination only — no reasoning (R10-safe, lives outside
//! the harness).

use tracing::{info, warn};

use crate::gateway::session_store::SessionStore;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

/// Honestly terminate a session's goal: block it from either non-terminal
/// state and delete its welded strategy so a stale plan can neither steer
/// later turns nor block a fresh start. Best-effort; returns whether a goal
/// was actually retired (terminal goals are never clobbered —
/// [`crate::goal::GoalStore::block_if_not_terminal`]'s contract).
///
/// `Paused` is retired alongside `Active` for the same reason the loop side
/// retires paused loops: the epoch bump (or delete) makes the old session
/// unreachable, and a goal can only be re-armed from its own session, so a
/// `Paused` goal left behind is one `goal(action='list')` keeps advertising
/// and nobody can ever restart. Consumers: the epoch-retirement seam below.
/// The `ResumeCoordinator` abandon paths use the narrower
/// [`block_abandonable_session_goal`] instead.
pub fn block_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_not_terminal(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "goal retired: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to retire session goal");
            false
        }
    }
}

/// Block a session goal ONLY when its recovery genuinely hung on a crashed
/// autonomous run — used by `ResumeCoordinator::abandon`. Delegates to
/// [`crate::goal::GoalStore::block_if_abandonable`] (Active-pursuit, not
/// task-barrier-parked) so a passive goal, or one parked on a task barrier
/// (woken by `GoalWakeService`), is left untouched. Deletes the welded
/// strategy on a real block, like [`block_session_goal`]. Returns whether a
/// goal was blocked.
pub fn block_abandonable_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_abandonable(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "abandonable goal blocked: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to block abandonable goal");
            false
        }
    }
}

/// Retire the `/btw` side session derived from `old_key`, awaiting the work.
///
/// **The single definition of what retiring a side session means.** Both entry
/// points below run exactly this; a second spelling of the sequence is how the
/// row and the event log drift apart.
///
/// # Why this is three deletions and not one
///
/// A side session's bytes are overwhelmingly *not* in its `sessions` row.
/// `btw::seed` copies the main transcript into the side session's own event log
/// (`fork::seed_events`) and the side run appends its turns there, so deleting
/// the row alone leaves the transcript in `session_events` plus its BM25 mirror
/// — still replayable, still searchable, and still re-materialised into a fresh
/// projection by `ProjectionReconciler` when a crashed side run left an
/// `Interrupted` marker. That is the same "it is unaddressable, so it is gone"
/// argument this whole seam exists to reject, one layer down.
///
/// So the order mirrors `sessions.delete`, for its stated reason: retire the
/// SSOT event log first, so a failure afterwards leaves a recoverable row
/// pointing at retired events rather than live events with no row.
///
/// # Three of that twin's five, and why the other two are out
///
/// `sessions.delete` also runs `purge_session_artifacts` and
/// `purge_session_snapshot`. Both are omitted here because a side session
/// cannot produce what they clean:
///
/// * **Artifacts.** A side turn is clamped to `ExecTier::Plan`
///   (`execution_engine::turn_permissions`), which refuses mutating tools, and
///   the only builtin that emits the `_media` key the artifact harvest consumes
///   is `canvas` — mutating, therefore denied. The one path not closed by
///   construction is a *read-only* MCP tool that returns `_media`; if that ever
///   becomes real, this is the line that has to grow.
/// * **Resume snapshot.** `resume.json` is written by the session-end
///   reflector, which a side session never reaches.
///
/// Both helpers are also private to `db_handlers::modify`, so wiring them would
/// mean lifting them — worth doing only for a reason better than symmetry, and
/// the reasons above say there is not one yet. Stated rather than left to the
/// reader, because a section that counts its own deletions against a twin
/// without saying why the twin has more is how the next reader concludes the
/// list is complete.
///
/// # Exactly one key, deliberately
///
/// This touches `side_key_for(old_key)` and nothing else. It must not grow into
/// a sweep over `SessionKey::Ephemeral`: that variant is also where background
/// sub-agent children and the OpenAI-compatible completions face live, so a
/// variant-wide sweep would delete work that is currently running. See the
/// `Ephemeral` doc in `routing::session_key`.
pub(crate) async fn retire_side_session_now(
    old_key: &SessionKey,
    cause: &str,
    store: &dyn SessionStore,
) {
    // A key that is already derived has no side session of its own; asking for
    // one would mint a phantom key nothing ever ran on.
    let Some(side) = crate::gateway::btw::side_session_of(old_key) else {
        return;
    };
    let side_str = side.to_key_string();

    // 1. The SSOT event log (and, through `retire_from`, its FTS mirror).
    //    `Ok(0)` when no event store is installed, which is what makes this
    //    safe in tests and in stores that never had one.
    let retired = match crate::session::store::retire_live_events(&side, 1).await {
        Ok(n) => n,
        Err(e) => {
            warn!(
                session = %side_str,
                cause,
                error = %e,
                "btw: side session transcript left behind — event retire failed"
            );
            0
        }
    };

    // 2. The execution plan keyed to the side session, for the same reason
    //    `SessionManager::delete_session` purges it: it is keyed by the session
    //    string, so a later session under this key would inherit it.
    crate::builtin_tools::scratchpad_registry::purge_session_scratchpad(&side_str).await;

    // 3. The row, which carries the incremental seed cursor.
    match store.delete_session(&side).await {
        Ok(result) if !result.deleted && retired == 0 => {
            // The overwhelmingly common case: this conversation never asked a
            // side question, so there was nothing anywhere. Not worth `info!`
            // on every `/new` — but not silent either: this is precisely the
            // reading a wrong derivation would produce, and the `&SessionKey`
            // parameter exists to prevent exactly that. `debug!` is what turns
            // "matched nothing" from unobservable into greppable.
            tracing::debug!(session = %side_str, cause, "btw: no side session to retire");
        }
        Ok(result) => info!(
            session = %side_str,
            cause,
            row_deleted = result.deleted,
            events_retired = retired,
            "btw: side session retired"
        ),
        // The one outcome that leaves residue behind, and the only reason
        // anyone would ever go looking for this log. Distinct from "nothing was
        // there" on purpose — collapsing the two would make the leak this
        // function exists to close indistinguishable from success.
        Err(e) => warn!(
            session = %side_str,
            cause,
            error = %e,
            "btw: side session row left behind — delete failed"
        ),
    }
}

/// Fire-and-forget [`retire_side_session_now`], for the surfaces that must not
/// wait on it.
///
/// Use this for a conversation whose key stops being the one `/btw` derives
/// from, or whose content the user just wiped:
///
/// * an epoch bump or a delete, via [`terminate_session_continuations`] — the
///   derived key changes, so the old side session becomes unaddressable;
/// * a compaction split, which bumps the epoch with no user action at all;
/// * `chat.clear` / `sessions.reset`, where the key is unchanged but the side
///   session still holds a **copy** of the transcript the user just cleared.
///
/// It is allowed to be best-effort because a missed retirement costs disk and
/// never a crossed side thread — the derivation includes the epoch.
///
/// # Precondition: a Tokio runtime must be current
///
/// The delete is spawned, so calling this (or [`terminate_session_continuations`],
/// which calls it) from outside a runtime panics with "there is no reactor
/// running". Every caller today is an `async fn` on the gateway runtime; a new
/// synchronous caller must either be on one or use
/// [`retire_side_session_now`] directly.
pub fn retire_side_session(
    old_key: &SessionKey,
    cause: &str,
    store: Option<Arc<dyn SessionStore>>,
) {
    let Some(store) = store else {
        return;
    };
    let key = old_key.clone();
    let cause = cause.to_string();
    tokio::spawn(async move {
        retire_side_session_now(&key, &cause, store.as_ref()).await;
    });
}

/// Stop the retired session's active loop, block its active goal, and delete
/// the `/btw` side session derived from it (all three best-effort), clearing
/// the loop's and goal's welded strategies so a stale plan can neither steer
/// later turns nor block a fresh start in the new epoch. `cause` names the
/// retiring surface for the stored stop reason / blocked note (e.g. `"/new"`,
/// `"sessions.new"`).
///
/// # Why the key is typed and the store is a parameter
///
/// `old_key` is a [`SessionKey`], not the key string the loop registry and
/// goal store are keyed by, because the side-session derivation
/// ([`crate::gateway::btw::side_key_for`]) hashes `to_key_string()` and must
/// hash *the same key the turn ran on*. Taking a string would put a re-parse
/// between the caller's key and the hash, and there are two spellings of that
/// parse (`SessionKey::parse` and `from_key_string`, which differ on legacy
/// input) — picking the wrong one derives a different side key, deletes
/// nothing, and reports success. Taking the key removes the class.
///
/// Like [`retire_side_session`], which it calls, this must run inside a Tokio
/// runtime: the side-session delete is spawned. A reader arriving here — the
/// entry point the module doc advertises — would otherwise not pass that
/// sentence.
///
/// `store` is passed in rather than fetched because there is deliberately no
/// process-global `SessionStore`, and adding one for this single caller would
/// be a second answer to "where is the session store". All three callers
/// already hold a handle. `None` means the caller genuinely has no store (the
/// inbound router's is optional), and costs at most the disk residue this
/// function exists to prevent.
pub fn terminate_session_continuations(
    old_key: &SessionKey,
    cause: &str,
    store: Option<Arc<dyn SessionStore>>,
) {
    // Canonical spelling: the loop registry and the goal store are keyed by
    // `to_key_string()`.
    let old_session = old_key.to_key_string();
    retire_side_session(old_key, cause, store);
    if let Some(reg) = crate::looping::global() {
        // Paused loops are retired too: the epoch bump makes the old session
        // unreachable, so a loop left quiet there could never be resumed —
        // leaving it `Paused` would only make `loop(action='list')` advertise a
        // loop nobody can ever restart.
        if matches!(
            reg.transition(
                &old_session,
                crate::looping::LoopStatus::Stopped,
                Some(format!("Session closed via {cause}")),
            ),
            crate::looping::TransitionOutcome::Applied { .. }
        ) {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::loop_key(&old_session));
            }
            info!(session = %old_session, cause, "session retired: loop stopped");
        }
    }
    block_session_goal(
        &old_session,
        &format!(
            "Session was closed via {cause} — re-set the goal in the new session to \
             continue pursuit."
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::gateway::btw::side_key_for;
    use crate::gateway::session_store::error::SessionStoreError;
    use crate::gateway::session_store::types::SessionMetadata;

    /// Assert that `key` still resolves, keeping the three answers apart.
    ///
    /// `matches!(…, Ok(Some(_)))` folds `Err` into whatever the failure message
    /// accuses the code of — the exact mistake [`wait_until_absent`] was
    /// written to avoid for the absence check. It matters more here than it
    /// looks: the accusation these callers print is "a sweep by variant would
    /// delete live sub-agent work", and a store that failed to answer is not
    /// evidence of a sweep. A guard that makes a false accusation is worse than
    /// no guard, because the next reader who re-runs it, sees green, and files
    /// it under flake has been taught to disbelieve the one thing standing
    /// between this function and a variant sweep.
    ///
    /// So `Err` is reported as `Err`, with the error, and only `Ok(None)` — a
    /// row that is genuinely gone — prints `swept`.
    ///
    /// History, because the next red should not be re-diagnosed from scratch:
    /// the bystander assertion was measured red on 2 of 5 clean-tree full-suite
    /// runs while it folded the three answers together, and 0 of 14 after this
    /// split plus giving each test its own agent id. That is evidence the
    /// mechanism is gone, not an explanation of it — nobody root-caused it, and
    /// splitting `Err` out cannot by itself turn a red green (it changes the
    /// message, not the outcome). If this fires again, the message now says
    /// which of the three answers arrived, which is the whole point.
    async fn assert_still_present(store: &dyn SessionStore, key: &SessionKey, swept: &str) {
        match store.get_metadata(key).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("{swept}"),
            Err(e) => panic!(
                "the store failed to answer for {} — that is not evidence of a \
                 sweep, it is a store error: {e}",
                key.to_key_string()
            ),
        }
    }

    /// Wait long enough that "nothing happened to this key" is a finding
    /// rather than a race.
    ///
    /// Presence cannot be polled for the way absence can — there is no edge to
    /// wait on — so an assertion that a row *survived* is only worth anything
    /// after a window comparable to the one a delete would need. Test 3 had
    /// this; test 2's bystander asserted the instant its sibling's delete
    /// landed, which a sweep visiting keys in order (`btw-…` sorts before
    /// `subagent-…`) would have slipped straight through.
    async fn settle_window() {
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Has this key stopped resolving?
    ///
    /// The retirement is fire-and-forget, so a single `yield_now()` does not
    /// observe it — the spawned task suspends on the store write. This polls
    /// with a bound, which keeps the guard honest: remove the retirement and
    /// it goes red rather than hanging.
    ///
    /// Absence is `Ok(None)`, never `Err`. An `Err` means the store failed to
    /// answer, which is a different fact and must not be read as "it is gone" —
    /// that is the shape that would let these tests pass for the wrong reason,
    /// so it is returned rather than swallowed.
    async fn wait_until_absent(
        store: &dyn SessionStore,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        let mut last = store.get_metadata(key).await;
        for _ in 0..200 {
            if matches!(last, Ok(None) | Err(_)) {
                return last;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            last = store.get_metadata(key).await;
        }
        last
    }

    /// The leak this module closes: nothing ever deleted a `/btw` side session.
    /// The epoch bump makes it unaddressable, so without a delete it is
    /// permanent disk residue no surface can list, reach or remove.
    #[tokio::test]
    async fn an_epoch_bump_retires_the_old_side_session() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("btw-epoch-bump");
        let side = side_key_for(&main);

        store
            .get_or_create(&side)
            .await
            .expect("side session row created");
        // Anchor before negating: without this, a broken fixture would make
        // the assertion below pass while proving nothing.
        assert!(
            matches!(store.get_metadata(&side).await, Ok(Some(_))),
            "fixture: the side session must exist before the retirement runs"
        );

        terminate_session_continuations(&main, "/new", Some(store.clone()));

        match wait_until_absent(store.as_ref(), &side).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                panic!("the old side session survived /new — it is now unreachable disk residue")
            }
            Err(e) => panic!("the store failed to answer, which is not the same as absence: {e}"),
        }
    }

    /// Retirement must delete exactly the derived key. `Ephemeral` is the
    /// catch-all for "a session that is not a conversation", and background
    /// sub-agent children live there too — a retirement that swept the variant
    /// would delete work that is currently running.
    #[tokio::test]
    async fn retirement_deletes_the_derived_key_and_no_other_ephemeral() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("btw-bystander");
        let side = side_key_for(&main);
        // Shaped like `subagent_spawner::ephemeral_for`: same agent, same
        // variant, different id.
        let bystander = SessionKey::Ephemeral {
            agent_id: "btw-bystander".to_string(),
            ephemeral_id: "subagent-bg-live-work".to_string(),
        };

        store.get_or_create(&side).await.expect("side row");
        store
            .get_or_create(&bystander)
            .await
            .expect("bystander row");

        terminate_session_continuations(&main, "sessions.delete", Some(store.clone()));

        assert!(
            matches!(wait_until_absent(store.as_ref(), &side).await, Ok(None)),
            "the derived side session should have been retired"
        );
        settle_window().await;
        assert_still_present(
            store.as_ref(),
            &bystander,
            "retirement reached an ephemeral session it does not own — a sweep by \
             variant would delete live sub-agent work",
        )
        .await;
    }

    /// A side key has no side session of its own, so retiring one must derive
    /// nothing and delete nothing.
    ///
    /// The observable is a row planted **at** the derived-of-derived key. A
    /// bare `side_key_for` here would derive that key and delete it; the
    /// production `side_session_of` returns `None` and never reaches the store.
    /// Without planting the row there is nothing to see — deleting a key that
    /// was never created is indistinguishable from not deriving it, which is
    /// how this guard would pass for the wrong reason.
    #[tokio::test]
    async fn retiring_a_side_key_derives_nothing_and_deletes_nothing() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("btw-idempotent");
        let side = side_key_for(&main);
        let derived_of_derived = side_key_for(&side);

        store.get_or_create(&side).await.expect("side row");
        store
            .get_or_create(&derived_of_derived)
            .await
            .expect("probe row at the derived-of-derived key");

        terminate_session_continuations(&side, "sessions.delete", Some(store.clone()));

        settle_window().await;
        assert_still_present(
            store.as_ref(),
            &derived_of_derived,
            "retiring a side key derived a second one and deleted it — the derivation \
             must be idempotent, or a side question's own retirement mints keys \
             nothing ever ran on",
        )
        .await;
        assert_still_present(
            store.as_ref(),
            &side,
            "retiring a side key must not delete the side session itself — the delete \
             targets the key derived FROM the retiring key",
        )
        .await;
    }

    /// The side session's bytes live in the event log, and deleting the row
    /// does not touch them.
    ///
    /// This is the leg the whole task is about, one level down: a row-only
    /// deletion looked like success for as long as anyone cared to check, and
    /// the log line said "retired". `btw::seed` copies a prefix of the main
    /// transcript into the **side** session's own event log, so the content a
    /// user would be alarmed to find surviving is here, not in the row — plus
    /// its BM25 mirror, which is what makes it findable rather than merely
    /// present.
    ///
    /// Mirrors `db_handlers::modify`'s
    /// `delete_retires_the_event_log_so_nothing_replays_or_searches`: install
    /// the process-wide store so `retire_live_events` is not a silent `Ok(0)`,
    /// then assert on **both** read paths. Asserting only that the call was
    /// made would be the "assert the effect arrived, not that the call
    /// happened" mistake — and without the installed store there is nothing to
    /// assert at all: delete the `retire_live_events` line and every other test
    /// here stays green.
    ///
    /// Own agent id, deliberately: `install_test_event_store` hands every
    /// caller the same process-wide instance, so two tests using
    /// `SessionKey::main("assistant")` would be retiring each other's events.
    #[tokio::test]
    async fn retirement_empties_the_side_transcript_and_its_search_mirror() {
        use crate::session::events::{MessageContent, SessionEvent};
        use crate::session::store::SessionEventStore;

        let events = crate::session::store::install_test_event_store();
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("btw-ssot-probe");
        let side = side_key_for(&main);

        store.get_or_create(&side).await.expect("side row");
        events
            .append(
                &side,
                1,
                &SessionEvent::UserMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text: "the side thread remembers hunter2".into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: 0,
                    synthetic: false,
                    author_user_id: None,
                },
                0,
            )
            .await
            .expect("seed the side transcript");

        // Anchor before negating: prove both read paths can see this content,
        // or the two assertions below would pass on an empty fixture.
        assert_eq!(
            events.load_all_events(&side).await.expect("replay").len(),
            1,
            "fixture: the side transcript must exist before the retirement runs"
        );
        assert_eq!(
            events
                .search_events(&side, "hunter2", 5)
                .await
                .expect("search")
                .len(),
            1,
            "fixture: the side transcript must be findable before the retirement runs"
        );

        terminate_session_continuations(&main, "/new", Some(store.clone()));
        assert!(
            matches!(wait_until_absent(store.as_ref(), &side).await, Ok(None)),
            "the side session row should have been retired"
        );
        settle_window().await;

        assert!(
            events
                .load_all_events(&side)
                .await
                .expect("replay")
                .is_empty(),
            "the retired side session still replays — the model can read a \
             transcript the log already called retired"
        );
        assert!(
            events
                .search_events(&side, "hunter2", 5)
                .await
                .expect("search")
                .is_empty(),
            "the retired side session is still searchable — its BM25 mirror \
             outlived the row that was reported deleted"
        );
    }

    // -----------------------------------------------------------------------
    // The census, as a rule rather than a list of names
    // -----------------------------------------------------------------------

    /// Files this rule cannot speak about, each for a reason re-derived from
    /// what the file *is* rather than from what it currently contains.
    ///
    /// Test code only. Nothing else is exempt — an entry naming a production
    /// file would be a licence, and this rule exists because the previous
    /// census was a sentence in a module doc that three surfaces ignored.
    fn is_test_only(path: &std::path::Path) -> bool {
        let p = path.to_string_lossy().replace('\\', "/");
        p.ends_with("/tests.rs")
            || p.contains("/tests/")
            || p.ends_with("_tests.rs")
            || p.contains("/test_utils")
    }

    /// Production source of one file: CR stripped (a CRLF checkout makes
    /// `\n`-anchored splits match nothing), the tests **module** dropped, and
    /// comment lines removed — a scanner that reads its own explanation is how
    /// a lexical guard certifies the thing it was written to catch.
    ///
    /// The cut is at the first `#[cfg(test)]` that introduces a `mod`, not at
    /// the first `#[cfg(test)]` of any kind. Those are not the same file:
    /// `agent_instance.rs:15` is a `#[cfg(test)] use …`, so cutting at the bare
    /// attribute left this scanner looking at fourteen lines of a thousand-line
    /// file and blind to the production `reset_session` far below it — one of
    /// the very call sites this guard was written to cover. A source-level
    /// guard that goes blind rather than red is worse than none, because its
    /// green reads as "you are wired" when it means "I cannot see you".
    ///
    /// A file whose tests module is spelled some other way is scanned whole.
    /// That errs toward a false positive, which is red and therefore visible —
    /// the opposite of the failure above.
    ///
    /// This used to end "Measured over the tree, no file does", which was false
    /// when it was written and is load-bearing: it is the stated reason this
    /// scanner may stay simple, and a later dispatch document quoted it rather
    /// than re-measuring. Re-measured 2026-08-22: **twelve** files under `src/`
    /// spell it with a visibility qualifier (`#[cfg(test)] pub(crate) mod tests`
    /// in `memory/embedding_provider.rs`, `gateway/handlers/auth/mod.rs`,
    /// `gateway/handlers/graph/mod.rs`, `builtin_tools/agent_manage/mod.rs`,
    /// `browser/mod.rs`, `acp/mock_server.rs`; `pub mod` or a differently-named
    /// module in `acp/mod.rs`, `sandbox/mod.rs`, `security/ssrf/dns.rs`,
    /// `teams/mod.rs`, `memory/session_search_summary/synthesizer.rs`, and
    /// `gateway/btw/guard_tests.rs`). Six of the twelve are read whole and six
    /// are cut later than a qualifier-aware scanner would cut them; either way
    /// this scanner reads MORE text, which is the safe direction, so no census
    /// here is currently lying — the measurement claim was. A qualifier-aware
    /// version lives in
    /// `gateway::btw::guard_tests::test_module_offset`; adopting it here would
    /// make those twelve files cut **earlier**, i.e. read less text, so it needs
    /// every census that depends on this helper re-measured and is deliberately
    /// not done as a drive-by.
    fn production_source(raw: &str) -> String {
        let no_cr = raw.replace('\r', "");
        let head = match test_module_offset(&no_cr) {
            Some(at) => &no_cr[..at],
            None => &no_cr,
        };
        head.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Byte offset of the first `#[cfg(test)]` whose next item is a `mod`
    /// (covering both `mod tests {` and `mod tests;`), or `None`.
    fn test_module_offset(src: &str) -> Option<usize> {
        const ATTR: &str = "#[cfg(test)]";
        let mut cursor = 0;
        while let Some(hit) = src[cursor..].find(ATTR) {
            let at = cursor + hit;
            let rest = src[at + ATTR.len()..].trim_start();
            if rest.starts_with("mod ") {
                return Some(at);
            }
            cursor = at + ATTR.len();
        }
        None
    }

    /// Count calls to `.<method>(` whose receiver is not the bare `self`.
    ///
    /// `self.close_session(...)` inside `impl SessionStore for SessionManager`
    /// is the trait forwarding to the inherent method — plumbing, not a surface
    /// that retires anything. A real surface calls it on something else
    /// (`manager.`, `sm.`, `self.session_store.`). Deriving that from the
    /// receiver keeps it a rule; naming the two files would make it a licence.
    fn calls_on_non_self(src: &str, method: &str) -> usize {
        let pat = format!(".{method}(");
        let mut count = 0;
        let mut cursor = 0;
        while let Some(hit) = src[cursor..].find(&pat) {
            let at = cursor + hit;
            let receiver: String = src[..at]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if receiver != "self" {
                count += 1;
            }
            cursor = at + pat.len();
        }
        count
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every production site that rolls a conversation's epoch, closes it, or
    /// wipes its content must reach a side-session retirement.
    ///
    /// Phrased over the *rule*, not over a list of caller names, because the
    /// thing this replaces was a list: `continuation_lifecycle`'s module doc
    /// said "the single seam every surface that retires a session must call"
    /// and then named three, while a fourth (the `session_new` tool) and a
    /// fifth (the compaction split) rolled epochs without it and nothing went
    /// red. A list cannot fail when the world grows past it; a rule can.
    ///
    /// # What it does not see, stated so nobody reads it as more than it is
    ///
    /// **One limit, and it is the only one.** A file passes by containing any
    /// retirement entry point *anywhere* in it, so this proves the wire exists
    /// in the file, not that it is on every path through it. Concretely: it
    /// names `session_split.rs`, `new_tool.rs`, `chat.rs` and
    /// `agent_instance.rs` when their wires are removed, but it would **not**
    /// have caught the `sessions.reset` gap on its own, because `modify.rs`
    /// already contained a retirement for its delete arm.
    ///
    /// Going finer would mean guessing where one `fn` ends inside a lexical
    /// scan, and a window whose boundary is a character count rather than the
    /// syntax it is scanning is its own well-documented failure. The
    /// behavioural guards above cover the wire actually firing; this one covers
    /// the wire existing at all, which is the failure that has now happened
    /// three times.
    ///
    /// Two limits this doc used to have and no longer does, recorded because
    /// each made the guard report green for a reason unrelated to the code:
    /// it cut the file at the first `#[cfg(test)]` of *any* kind rather than at
    /// the tests module (see [`production_source`]), and it did not count
    /// `delete_session`, so the "delete" third of the seam's own contract was
    /// covered only by coincidence.
    #[test]
    fn every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        assert!(
            files.len() > 500,
            "scanner found only {} .rs files under {} — it is not reading the tree",
            files.len(),
            root.display()
        );

        const RETIREMENTS: [&str; 4] = [
            "terminate_session_continuations(",
            "retire_side_session(",
            "retire_side_session_now(",
            "retire_superseded(",
        ];

        let mut sites = 0usize;
        let mut with_markers = Vec::new();
        let mut unwired = Vec::new();

        for path in files {
            if is_test_only(&path) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let src = production_source(&raw);

            // `with_next_epoch()` with empty parens is a *call*; the definition
            // reads `with_next_epoch(&self)` and so is not matched.
            // The seam's contract names three categories — epoch bump,
            // delete, content wipe. `delete_session` is here because the
            // delete category was previously covered only by accident:
            // `modify.rs` happens to also contain a `.reset_session(`, so a
            // delete surface in a file of its own was invisible.
            let n = src.matches("with_next_epoch()").count()
                + calls_on_non_self(&src, "register_epoch")
                + calls_on_non_self(&src, "close_session")
                + calls_on_non_self(&src, "reset_session")
                + calls_on_non_self(&src, "delete_session");
            if n == 0 {
                continue;
            }
            sites += n;
            with_markers.push(path.clone());
            if !RETIREMENTS.iter().any(|r| src.contains(r)) {
                unwired.push(format!("{} ({n} site(s))", path.display()));
            }
        }

        // Self-check: distinguishes "every file is wired" from "the scanner
        // stopped seeing anything", which read identically in the report that
        // let two of these through.
        // Tight against today's tree (8 files / 12 sites), so shrinkage — a
        // marker that stops matching, a split that swallows a file — reddens
        // instead of quietly narrowing the guard's world.
        assert!(
            with_markers.len() >= 8 && sites >= 12,
            "census went blind: {} file(s) / {} site(s) — the markers or the \
             cfg(test) split stopped matching",
            with_markers.len(),
            sites
        );

        assert!(
            unwired.is_empty(),
            "these production files roll a session's epoch, close it, or wipe \
             its content without reaching a side-session retirement — a `/btw` \
             side session they orphan is unaddressable and nothing will ever \
             delete it:\n  {}",
            unwired.join("\n  ")
        );
    }

    /// Every face of "start a new conversation" — bump the epoch AND retire the
    /// predecessor — must first ask whether the key can actually roll.
    ///
    /// `with_epoch` / `with_next_epoch` end in `_ => {}`: for Group / Task /
    /// Subagent / Ephemeral the "new" key IS the old one, so the sequence
    /// (terminate continuations → close → get_or_create) stops the running
    /// loop, deletes the `/btw` side session and stamps the still-live row
    /// `status: "closed"`, then hands the caller back the key it started with
    /// and reports success. Only the `session_new` tool checked; the channel
    /// `/new` command and the `sessions.new` RPC did not, and a `/new` typed in
    /// any group chat reached both of the unguarded ones.
    ///
    /// Phrased over the rule, not over the three file names, for the reason the
    /// census above is: the previous state of this family WAS a list — the one
    /// correct predicate lived in `new_tool.rs` with its own passing test, and
    /// that made the family read as covered.
    ///
    /// Trigger is deliberately "bumps AND terminates", not "bumps": a site that
    /// mints a successor without retiring the predecessor (`AgentRouter::route`
    /// hands out a fresh key for a keyless request; `agent_resolver` resolves
    /// an existing epoch) is not this verb and must not be forced to answer
    /// this question.
    #[test]
    fn every_face_that_rolls_and_retires_asks_whether_the_key_can_roll() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&root, &mut files);

        let mut triggered = Vec::new();
        let mut ungated = Vec::new();
        for path in files {
            if is_test_only(&path) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let src = production_source(&raw);
            let bumps =
                src.contains("with_next_epoch()") || calls_on_non_self(&src, "with_epoch") > 0;
            let retires = src.contains("terminate_session_continuations(");
            if !(bumps && retires) {
                continue;
            }
            triggered.push(path.clone());
            if !src.contains("rolls_to_new_epoch()") {
                ungated.push(path.display().to_string());
            }
        }

        // Self-check: "no ungated face" and "the scanner stopped finding faces"
        // are the same green without this. Three faces today.
        assert!(
            triggered.len() >= 3,
            "census went blind: only {} file(s) both bump an epoch and retire \
             the predecessor — the markers or the cfg(test) split stopped matching",
            triggered.len()
        );

        assert!(
            ungated.is_empty(),
            "these production files roll a conversation to a new epoch AND \
             retire the old one without first asking `SessionKey::\
             rolls_to_new_epoch()` — on a Group/Task/Subagent/Ephemeral key the \
             bump is a clone, so they destroy the live session and report a \
             fresh one:\n  {}",
            ungated.join("\n  ")
        );
    }
}
