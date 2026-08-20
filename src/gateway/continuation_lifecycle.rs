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
/// # Exactly one key, deliberately
///
/// This touches `side_key_for(old_key)` and nothing else. It must not grow into
/// a sweep over `SessionKey::Ephemeral`: that variant is also where background
/// sub-agent children and the OpenAI-compatible completions face live, so a
/// variant-wide sweep would delete work that is currently running. See the
/// `Ephemeral` doc in `routing::session_key`.
pub(crate) async fn retire_side_session_now(old_key: &SessionKey, cause: &str, store: &dyn SessionStore) {
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
pub fn retire_side_session(old_key: &SessionKey, cause: &str, store: Option<Arc<dyn SessionStore>>) {
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
        let main = SessionKey::main("assistant");
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
        let main = SessionKey::main("assistant");
        let side = side_key_for(&main);
        // Shaped like `subagent_spawner::ephemeral_for`: same agent, same
        // variant, different id.
        let bystander = SessionKey::Ephemeral {
            agent_id: "assistant".to_string(),
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
        assert!(
            matches!(store.get_metadata(&bystander).await, Ok(Some(_))),
            "retirement reached an ephemeral session it does not own — a sweep by \
             variant would delete live sub-agent work"
        );
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
        let main = SessionKey::main("assistant");
        let side = side_key_for(&main);
        let derived_of_derived = side_key_for(&side);

        store.get_or_create(&side).await.expect("side row");
        store
            .get_or_create(&derived_of_derived)
            .await
            .expect("probe row at the derived-of-derived key");

        terminate_session_continuations(&side, "sessions.delete", Some(store.clone()));

        settle_window().await;
        assert!(
            matches!(store.get_metadata(&derived_of_derived).await, Ok(Some(_))),
            "retiring a side key derived a second one and deleted it — the derivation \
             must be idempotent, or a side question's own retirement mints keys \
             nothing ever ran on"
        );
        assert!(
            matches!(store.get_metadata(&side).await, Ok(Some(_))),
            "retiring a side key must not delete the side session itself — the delete \
             targets the key derived FROM the retiring key"
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
    /// `\n`-anchored splits match nothing), the `#[cfg(test)]` tail dropped,
    /// and comment lines removed — a scanner that reads its own explanation is
    /// how a lexical guard certifies the thing it was written to catch.
    fn production_source(raw: &str) -> String {
        let no_cr = raw.replace('\r', "");
        let head = no_cr.split("#[cfg(test)]").next().unwrap_or("").to_string();
        head.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
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
    /// A file passes by containing any retirement entry point *anywhere* in it,
    /// so this proves the wire exists in the file, not that it is on every path
    /// through it. Concretely: it names `session_split.rs`, `new_tool.rs` and
    /// `chat.rs` when their wires are removed, but it would **not** have caught
    /// the `sessions.reset` gap on its own, because `modify.rs` already
    /// contained a retirement for its delete arm.
    ///
    /// Going finer would mean guessing where one `fn` ends inside a lexical
    /// scan, and a window whose boundary is a character count rather than the
    /// syntax it is scanning is its own well-documented failure. The three
    /// behavioural guards above cover the wire actually firing; this one covers
    /// the wire existing at all, which is the failure that has now happened
    /// three times.
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
            let n = src.matches("with_next_epoch()").count()
                + calls_on_non_self(&src, "register_epoch")
                + calls_on_non_self(&src, "close_session")
                + calls_on_non_self(&src, "reset_session");
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
        assert!(
            with_markers.len() >= 6 && sites >= 8,
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
}
