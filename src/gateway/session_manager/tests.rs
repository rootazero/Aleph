#![allow(deprecated)]

use super::*;
use tempfile::tempdir;

fn test_config(path: PathBuf) -> SessionManagerConfig {
    SessionManagerConfig {
        db_path: path,
        max_messages: 10,
        compaction_keep: 5,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_session_creation() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    let meta = manager.get_or_create(&key).await.unwrap();

    assert_eq!(meta.agent_id, "test");
    assert_eq!(meta.session_type, "main");
    assert_eq!(meta.message_count, 0);
}

#[tokio::test]
async fn test_message_operations() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Add messages
    manager.add_message(&key, "user", "Hello").await.unwrap();
    manager
        .add_message(&key, "assistant", "Hi there!")
        .await
        .unwrap();

    let history = manager.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

#[tokio::test]
async fn history_page_cursor_filters_by_timestamp() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "one").await.unwrap();
    manager.add_message(&key, "assistant", "two").await.unwrap();
    manager.add_message(&key, "user", "three").await.unwrap();

    let now = chrono::Utc::now();

    // No cursor → identical to plain get_history (all three).
    let all = manager.history_page(&key, None, None).await.unwrap().rows;
    assert_eq!(all.len(), 3);

    // Cursor in the future → every message is strictly older, so all survive.
    let before_future = manager
        .history_page(&key, None, Some(now + chrono::Duration::hours(1)))
        .await
        .map(|p| p.rows)
        .unwrap();
    assert_eq!(before_future.len(), 3);

    // Cursor far in the past → nothing is older than it.
    let before_past = manager
        .history_page(&key, None, Some(now - chrono::Duration::hours(1)))
        .await
        .map(|p| p.rows)
        .unwrap();
    assert!(before_past.is_empty());

    // Limit still windows the cursor-filtered set (most-recent `limit`).
    let windowed = manager
        .history_page(&key, Some(2), Some(now + chrono::Duration::hours(1)))
        .await
        .map(|p| p.rows)
        .unwrap();
    assert_eq!(windowed.len(), 2);
}

/// The Rust and SQL spellings of the seconds/millisecond boundary must agree.
///
/// Neither is expressible in terms of the other — one is a Rust `fn`, the other
/// a string interpolated into a query — so the only check that can see them
/// drift is evaluating both over the same values. They share the boundary
/// constant, which is why the interesting inputs are the ones that straddle it
/// and the ones where SQLite's own semantics could differ from Rust's (integer
/// division / negative operands / `abs`).
#[test]
fn the_sql_and_rust_spellings_of_stamp_millis_agree() {
    use crate::gateway::session_store::types::{stamp_millis, SECONDS_MILLIS_BOUNDARY};

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE messages (timestamp INTEGER);")
        .unwrap();
    let expr = SessionManager::stamp_millis_sql();

    for raw in [
        0_i64,
        1,
        -1,
        1_785_062_232,               // seconds, a real stamp
        1_785_062_232_000,           // the same instant in milliseconds
        SECONDS_MILLIS_BOUNDARY - 1, // just below the boundary
        SECONDS_MILLIS_BOUNDARY,     // exactly at it
        -SECONDS_MILLIS_BOUNDARY,    // and its mirror, which `abs` catches
        -(SECONDS_MILLIS_BOUNDARY - 1),
        -1_785_062_232,
    ] {
        conn.execute("DELETE FROM messages", []).unwrap();
        conn.execute("INSERT INTO messages (timestamp) VALUES (?)", [raw])
            .unwrap();
        let from_sql: i64 = conn
            .query_row(&format!("SELECT {expr} FROM messages"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            from_sql,
            stamp_millis(raw),
            "SQL and Rust disagree on raw stamp {raw}; the two normalizers have \
             drifted and the cursor now means different things on the two backends"
        );
    }
}

/// Every module that writes SQL against `messages`.
///
/// One list, shared by both halves of the ordering discipline below, because
/// two lists is how a file joins one guard and not the other. A file that stops
/// containing SQL fails each guard's self-check rather than passing vacuously.
const MESSAGE_SQL_FILES: [(&str, &str); 5] = [
    ("session_manager/ops/crud.rs", include_str!("ops/crud.rs")),
    ("session_manager/ops/query.rs", include_str!("ops/query.rs")),
    (
        "session_manager/ops/identity.rs",
        include_str!("ops/identity.rs"),
    ),
    (
        "session_manager/ops/modify.rs",
        include_str!("ops/modify.rs"),
    ),
    (
        "session_store/sqlite_backend/mod.rs",
        include_str!("../session_store/sqlite_backend/mod.rs"),
    ),
];

/// `\r` first (this repo is checked out CRLF on Windows, and a scanner that
/// anchors on `\n` finds nothing there while staying green), then comment
/// lines — a doc comment naming a pattern is documentation, not code, and the
/// whole point of these guards is that prose and code are separate. Then runs
/// of whitespace and Rust string continuations collapse to single spaces so a
/// wrapped `ORDER BY` still reads as one phrase.
fn flatten(src: &str) -> String {
    let body = src
        .replace('\r', "")
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = String::with_capacity(body.len());
    let mut prev_ws = false;
    for ch in body.chars() {
        if ch == '\\' {
            continue;
        }
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

/// A DELETE names its rows through the same ordered `SELECT` the reads use —
/// never by translating a boundary row's id into a second statement.
///
/// **The spelling this bans would be correct today.** The transcript's order is
/// `id` (`SessionStore::history_page`), so `id > <boundary>` and "everything
/// past the first N in reader order" name the same set. It is banned because
/// the two spellings are only equal by coincidence of the current order, and
/// this repo has now changed that order twice:
///
///   * `messages.timestamp` was the insert clock, so ranking by it WAS ranking
///     by `id`, and `compact_session` / `truncate_messages` translated their
///     boundary into `id < ?` / `id > ?` correctly.
///   * `add_message_full` stopped overwriting the producer's stamp, and the two
///     came apart: with rows `(id 1, t 300)`, `(id 2, t 100)`, `(id 3, t 200)`
///     and `keep = 2` the boundary landed on id 2 and `id < 2` deleted the
///     NEWEST message while the oldest survived — `Ok(1)`, no error.
///   * The order was then settled on `id`, and they coincide again.
///
/// A boundary translated between statements is correct only while some
/// unwritten invariant holds; one ordered `SELECT` that names its own victims
/// has no second statement to disagree with. That is the same move round-6 made
/// on `history_page` — a thing with an order to get wrong became a thing with
/// one call.
///
/// It is free: both sites already name their victims this way, so there are
/// ZERO occurrences and no allowlist — and an allowlist here would be a second
/// place that decides which deletes may be ordered wrongly.
///
/// A statement that legitimately needs a row-id range (there is none today)
/// should name its rows through the ordered subquery too; if one ever genuinely
/// cannot, the exemption belongs in the code as a differently-shaped predicate,
/// not as an entry here.
#[test]
fn no_delete_boundary_is_applied_in_a_different_order_than_it_was_ranked() {
    let mut delete_sites = 0_usize;
    for (label, src) in MESSAGE_SQL_FILES {
        let flat = flatten(src);
        // Self-check: a scanner that quietly stops seeing DELETEs reports the
        // same clean result as a codebase with no violations.
        delete_sites += flat.matches("DELETE FROM ").count();
        for pattern in ["id < ?", "id > ?", "id <= ?", "id >= ?"] {
            assert!(
                !flat.contains(pattern),
                "{label} translates a boundary row into `{pattern}` instead of \
                 letting one ordered SELECT name the rows it deletes. That is \
                 two statements that must agree about the order, and this repo \
                 has changed the transcript's order twice — the last time, a \
                 boundary chosen in event order and applied in insert order \
                 deleted the NEWEST message in the session and returned \
                 success. Name the rows with the ordered SELECT itself — \
                 `id IN (SELECT id ... ORDER BY id ... LIMIT -1 OFFSET ?)` — \
                 the way `compact_session` and `truncate_messages` do."
            );
        }
    }
    assert!(
        delete_sites >= 6,
        "only {delete_sites} `DELETE FROM` sites found across {} files; this \
         guard used to see more, so either statements were removed or \
         `flatten` stopped matching them",
        MESSAGE_SQL_FILES.len()
    );
}

/// Every SQL statement that READS the `timestamp` column as a quantity must
/// read it through [`SessionManager::stamp_millis_sql`], never bare.
///
/// Since the transcript's order became `id`, the only such statement is the
/// `before` cursor — but the ban stays whole-column rather than
/// cursor-specific, because the property is about the column, not about which
/// clause happens to touch it this month.
///
/// The column holds two units (see [`MessageRecord::timestamp`]). It used to be
/// uniformly seconds on the SQLite half — `add_message_full` overwrote whatever
/// the producer stamped — so ordering by the raw column was *then* correct, and
/// that was the point of writing this guard before repairing the adjacent
/// fidelity bug: the correctness was on loan from an invariant nothing
/// enforced, and two of the statements below choose the boundary of a DELETE
/// (`truncate_messages` behind `/undo`, `compact_session`). The fidelity bug is
/// repaired now, so the loan has been called in: the column really is mixed and
/// a raw comparison really does keep and drop the wrong rows.
///
/// Prose in `MessageRecord::timestamp`'s doc already said the unit was
/// ambiguous. Prose does not stop the next sincere fixer; this does.
///
/// Its sibling
/// [`no_delete_boundary_is_applied_in_a_different_order_than_it_was_ranked`]
/// covers the DELETEs, and
/// [`no_message_query_orders_by_the_stamp`] covers the orderings — the stamp is
/// no longer allowed to be an order at all, normalized or not.
///
/// Source-level because the property is about the SQL that is *written*, not
/// about any one query's output: a statement ordering by the raw column returns
/// identical rows on today's uniform data, so no runtime test on this database
/// can tell the two spellings apart.
///
/// [`MessageRecord::timestamp`]: crate::gateway::session_store::types::MessageRecord::timestamp
#[test]
fn no_message_query_ranks_by_the_raw_timestamp_column() {
    let files = MESSAGE_SQL_FILES;

    // `timestamp,` (a column in a SELECT list) and `timestamp)` (inside
    // `stamp_millis_sql` itself) are projections, not rankings, and are not
    // matched by any of these.
    let banned = ["ORDER BY timestamp", "timestamp <", "timestamp >"];

    let mut ranked_sites = 0_usize;
    for (label, src) in files {
        let flat = flatten(src);
        // Self-check: a scanner that quietly stops seeing SQL reports the same
        // clean result as a codebase with no violations.
        let order_bys = flat.matches("ORDER BY ").count();
        assert!(
            order_bys > 0,
            "{label} no longer contains any `ORDER BY` — this guard is scanning \
             a file with no SQL left in it and would pass vacuously. Either the \
             queries moved (update the list) or the file was emptied."
        );
        ranked_sites += order_bys;
        for pattern in banned {
            assert!(
                !flat.contains(pattern),
                "{label} ranks `messages` by the raw `timestamp` column \
                 (`{pattern}`). That column holds seconds in some rows and \
                 milliseconds in others, so the raw comparison sorts every \
                 millisecond row above every seconds row regardless of when \
                 either happened. Rank through \
                 `SessionManager::stamp_millis_sql()` — the one place the \
                 seconds/milliseconds boundary is applied in SQL — the way \
                 `get_history` and `history_page` already do."
            );
        }
    }
    assert!(
        ranked_sites >= 8,
        "only {ranked_sites} ranked SQL sites found across {} files; this guard \
         used to see more, so either queries were removed or `flatten` stopped \
         matching them",
        files.len()
    );
}

/// No statement may ORDER `messages` by the stamp — normalized or raw.
///
/// The transcript's order is the order its rows were recorded: `messages.id`
/// here, file position in `transcript.jsonl`. The reasoning is on
/// [`SessionStore::history_page`]; what this guard adds is that the SQL half
/// cannot quietly re-acquire a second opinion. It had one for four rounds —
/// SQLite ranked `(stamp_millis_sql, id)` while the file store served and
/// deleted by position — and the two agreed only because `add_message_full`
/// was overwriting every producer's stamp with the insert clock.
///
/// Source-level because no runtime test on THIS database can see it: every
/// production producer writes a stamp that is monotonic in append order, so a
/// stamp-ordered store and a position-ordered store return identical rows for
/// all real data. The divergence needs a hand-built transcript to observe —
/// which is what `delete_boundary_order_tests` and
/// `transcript_order_tests` below build, and which is why the source rule and
/// the runtime tests are both needed rather than either alone.
///
/// [`SessionStore::history_page`]: crate::gateway::session_store::SessionStore::history_page
#[test]
fn no_message_query_orders_by_the_stamp() {
    let mut recording_order_sites = 0_usize;
    for (label, src) in MESSAGE_SQL_FILES {
        let flat = flatten(src);
        recording_order_sites += flat.matches("ORDER BY id").count();
        for pattern in ["ORDER BY {stamp}", "ORDER BY (CASE"] {
            assert!(
                !flat.contains(pattern),
                "{label} orders `messages` by the timestamp (`{pattern}`). The \
                 transcript's order is the order its rows were recorded — \
                 `ORDER BY id` here, file position in the file backend. \
                 Ordering by the stamp gives this backend a second opinion \
                 about the same conversation, and two of these statements \
                 choose the victims of a DELETE. See \
                 `SessionStore::history_page` for why recording order won."
            );
        }
    }
    // The ban above keys on the spelling `{stamp}`, which is a BINDING NAME —
    // `let s = Self::stamp_millis_sql()` renames it and walks straight past.
    // (Found by mutation: the renamed spelling tripped only the self-check
    // below, and only because it happened to remove two `ORDER BY id` sites
    // with it.) The thing that cannot be renamed is the call itself, so the
    // real rule is a COUNT: `stamp_millis_sql` is called from exactly one
    // place, the `before` cursor in `history_sql`. Ordering by it needs a
    // second call site, and there is no spelling of a second call site that
    // this does not see.
    //
    // A genuinely new predicate over this column would also trip it. That is
    // intended: it is a column with two units, one caller, and a written
    // reason — a second one should have to say so out loud.
    let calls: usize = MESSAGE_SQL_FILES
        .iter()
        .map(|(_, src)| {
            flatten(src)
                .matches("stamp_millis_sql()")
                .count()
                // `pub(crate) fn stamp_millis_sql() -> String` is the
                // definition, not a call.
                .saturating_sub(usize::from(src.contains("fn stamp_millis_sql()")))
        })
        .sum();
    assert_eq!(
        calls, 1,
        "`stamp_millis_sql()` is called from {calls} places; it must be called \
         from exactly one — the `before` cursor in `history_sql`. Ranking by \
         it is what this guard exists to prevent, and the ban above can be \
         evaded by renaming the interpolated binding, so the count is the part \
         that holds. If you are adding a legitimate second PREDICATE over \
         `messages.timestamp`, say why here and raise the number."
    );

    // Self-check: a scanner that stops seeing the orderings reports the same
    // clean result as a codebase with none.
    assert!(
        recording_order_sites >= 8,
        "only {recording_order_sites} `ORDER BY id` sites found across {} \
         files; this guard used to see 8, so either statements were removed or \
         `flatten` stopped matching them",
        MESSAGE_SQL_FILES.len()
    );
}

/// The runtime half of the guards above: what a boundary applied in the wrong
/// order actually did to a session.
///
/// A source guard can only say "this spelling is banned". These say what the
/// banned spelling produced, in the one arrangement that tells the two orders
/// apart — a transcript whose event order differs from its insert order, which
/// is what any reconciler, backfill or import creates and what
/// `add_message_full` now faithfully records.
#[cfg(test)]
mod delete_boundary_order_tests {
    use super::*;
    use crate::gateway::session_store::types::MessageRecord;
    use crate::gateway::session_store::SessionStore;

    /// Real epoch seconds, not 100/200/300: `stamp_millis` classifies by
    /// magnitude, so a fixture built from toy numbers is a fixture that has
    /// never been on the interesting side of any threshold this code reads.
    const T_OLD: i64 = 1_700_000_100;
    const T_MID: i64 = 1_700_000_200;
    const T_NEW: i64 = 1_700_000_300;

    fn row(content: &str, timestamp: i64) -> MessageRecord {
        MessageRecord {
            id: content.into(),
            role: "user".into(),
            content: content.into(),
            timestamp,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// Recording order `new, old, mid` — ids 1, 2, 3 — with stamps that put
    /// them in a different order (`old, mid, new`), so the two candidate orders
    /// disagree about every position.
    ///
    /// No production producer writes a transcript like this: the projector
    /// stamps `created_at_ms`, monotonic in `seq`, and the other two stamp
    /// `Utc::now()`. That is the point — the divergence between "the order rows
    /// were recorded" and "the order their stamps claim" is unobservable on
    /// real data, so it has to be built by hand or not seen at all.
    async fn session_whose_events_are_out_of_insert_order(
        temp: &tempfile::TempDir,
        keep: usize,
    ) -> (SessionManager, SessionKey) {
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("test.db"),
            // High enough that nothing auto-compacts underneath the test.
            max_messages: 100,
            compaction_keep: keep,
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::main("out-of-order");
        manager.get_or_create(&key).await.unwrap();
        for (content, at) in [("new", T_NEW), ("old", T_OLD), ("mid", T_MID)] {
            manager
                .append_message(&key, row(content, at))
                .await
                .unwrap();
        }
        (manager, key)
    }

    async fn contents(manager: &SessionManager, key: &SessionKey) -> Vec<String> {
        manager
            .get_history(key, None)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect()
    }

    /// The transcript is served in the order its rows were recorded.
    ///
    /// This is the whole decision in one assertion, and it is the premise the
    /// two DELETE tests below stand on: "the newest `keep`" and "the oldest
    /// `keep`" are only meaningful once "the order" has one answer.
    #[tokio::test]
    async fn the_transcript_is_served_in_the_order_it_was_recorded() {
        let temp = tempdir().unwrap();
        let (manager, key) = session_whose_events_are_out_of_insert_order(&temp, 5).await;

        let served = contents(&manager, &key).await;

        assert_eq!(
            served,
            vec!["new".to_string(), "old".to_string(), "mid".to_string()],
            "the transcript came back as {served:?}. It must come back in the \
             order its rows were recorded; `old, mid, new` is this backend \
             sorting by the producers' stamps, which is a second opinion about \
             the same conversation that the file backend cannot hold."
        );
    }

    /// Compaction keeps the last `compaction_keep` messages of the transcript.
    ///
    /// **This reverses a previous round's assertion**, which had a name, a
    /// reason and a passing test: `compaction_drops_the_oldest_not_the_earliest_inserted`
    /// kept `["mid", "new"]` here, because the transcript's order was then the
    /// producers' stamps. The order is now the order rows were recorded
    /// (`SessionStore::history_page`), so the tail is `["old", "mid"]` and the
    /// row compaction drops is `new` — the first one recorded. Nothing about
    /// the old assertion was wrong for the order it was written under; the
    /// order changed.
    ///
    /// What is NOT reversed, and is what this test still protects: the victims
    /// are named by the same ordered `SELECT` that defines the order. The
    /// spelling that preceded both rounds took the id of the boundary row and
    /// deleted `id < that`, which under stamp ranking destroyed the most recent
    /// message in the conversation and returned `Ok(1)`.
    #[tokio::test]
    async fn compaction_keeps_the_tail_of_the_recorded_order() {
        let temp = tempdir().unwrap();
        let (manager, key) = session_whose_events_are_out_of_insert_order(&temp, 2).await;

        let deleted = manager.compact_session(&key).await.unwrap();

        assert_eq!(
            deleted, 1,
            "expected exactly the one row over the keep line"
        );
        let kept = contents(&manager, &key).await;
        assert_eq!(
            kept,
            vec!["old".to_string(), "mid".to_string()],
            "compaction kept {kept:?} — it must keep the LAST two rows of the \
             transcript. `mid, new` is the keep line being drawn by the \
             producers' stamps instead."
        );
    }

    /// `/undo` keeps the first `keep_count` rows of the transcript — the same
    /// cut the file backend makes with `drain(keep_count..)`, which is the
    /// reason this order was chosen over the stamps.
    ///
    /// Reversed from `undo_drops_the_newest_not_the_latest_inserted` for the
    /// reason given on the compaction test above.
    ///
    /// Two defects met here originally. The boundary landed on id 3 ("mid") and
    /// `id > 3` matched nothing, so the tail survived; and the call could not
    /// get that far anyway, because the FTS transaction was shadowed instead of
    /// committed and the second `BEGIN` was refused — `session.truncate` had
    /// never once succeeded on this backend. That is what `.expect("truncate
    /// must reach the database at all")` is still here for.
    #[tokio::test]
    async fn undo_keeps_the_head_of_the_recorded_order() {
        let temp = tempdir().unwrap();
        let (manager, key) = session_whose_events_are_out_of_insert_order(&temp, 5).await;

        let result = manager
            .truncate_messages(&key, 2)
            .await
            .expect("truncate must reach the database at all");

        assert_eq!(result.messages_removed, 1, "expected the single last row");
        let kept = contents(&manager, &key).await;
        assert_eq!(
            kept,
            vec!["new".to_string(), "old".to_string()],
            "`/undo` kept {kept:?} — it must keep the FIRST two rows of the \
             transcript. `old, mid` is the cut being made by the producers' \
             stamps instead."
        );
    }

    /// `keep_count == 0` clears the session. It used to be a hand-written arm;
    /// `OFFSET 0` names every row, so the arm went away and this is what says
    /// the behaviour did not go with it.
    #[tokio::test]
    async fn undo_to_zero_clears_the_session() {
        let temp = tempdir().unwrap();
        let (manager, key) = session_whose_events_are_out_of_insert_order(&temp, 5).await;

        let result = manager.truncate_messages(&key, 0).await.unwrap();

        assert_eq!(result.messages_removed, 3);
        assert!(contents(&manager, &key).await.is_empty());
    }
}

#[tokio::test]
async fn test_session_reset() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "Test").await.unwrap();

    assert!(manager.reset_session(&key).await.unwrap());

    let history = manager.get_history(&key, None).await.unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn test_compaction() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Add more messages than max_messages
    for i in 0..15 {
        manager
            .add_message(&key, "user", &format!("Message {}", i))
            .await
            .unwrap();
    }

    // Compaction should have happened automatically
    let history = manager.get_history(&key, None).await.unwrap();
    assert!(history.len() <= 10); // Should be at most max_messages after compaction
}

#[tokio::test]
async fn test_list_sessions() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    manager
        .get_or_create(&SessionKey::main("agent1"))
        .await
        .unwrap();
    manager
        .get_or_create(&SessionKey::main("agent2"))
        .await
        .unwrap();
    manager
        .get_or_create(&SessionKey::peer("agent1", "peer1"))
        .await
        .unwrap();

    let all = manager.list_sessions(None).await.unwrap();
    assert_eq!(all.len(), 3);

    let agent1_only = manager.list_sessions(Some("agent1")).await.unwrap();
    assert_eq!(agent1_only.len(), 2);
}

#[tokio::test]
async fn test_set_source_channel_records_and_surfaces_origin() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Fresh session: origin is the "unknown" sentinel → surfaced as None.
    let before = manager.list_sessions(None).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].origin_channel(), None);

    // Stamp the originating channel + conversation; both must be surfaced via
    // the read path.
    manager
        .set_source_channel(&key, "telegram", Some("chat-77"))
        .await
        .unwrap();
    let after = manager.list_sessions(None).await.unwrap();
    assert_eq!(
        after[0].origin_channel(),
        Some("telegram".to_string()),
        "first stamp must record the real origin"
    );
    assert_eq!(
        after[0].origin_conversation(),
        Some("chat-77".to_string()),
        "first stamp must capture the origin conversation id for reply fan-out"
    );

    // Idempotent: a later continuation from a different surface must NOT clobber
    // the recorded origin (this is what keeps multi-end continuity honest).
    manager
        .set_source_channel(&key, "gui:chat", None)
        .await
        .unwrap();
    let still = manager.list_sessions(None).await.unwrap();
    assert_eq!(
        still[0].origin_channel(),
        Some("telegram".to_string()),
        "second stamp must not clobber the existing origin"
    );
    assert_eq!(
        still[0].origin_conversation(),
        Some("chat-77".to_string()),
        "second stamp must not clobber the origin conversation either"
    );
}

#[tokio::test]
async fn test_set_source_channel_skips_empty_and_unknown_sentinel() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Empty / sentinel inputs are no-ops: origin stays unset (None).
    manager.set_source_channel(&key, "   ", None).await.unwrap();
    manager
        .set_source_channel(&key, "unknown", Some("x"))
        .await
        .unwrap();
    let sessions = manager.list_sessions(None).await.unwrap();
    assert_eq!(sessions[0].origin_channel(), None);
    assert_eq!(sessions[0].origin_conversation(), None);
}

#[test]
fn test_session_identity_meta_default() {
    let meta = SessionIdentityMeta::default();
    assert_eq!(meta.role, Role::Owner);
    assert_eq!(meta.identity_id, "owner");
    assert!(meta.scope.is_none());
    assert_eq!(meta.source_channel, "unknown");
}

#[test]
fn test_session_identity_meta_owner_factory() {
    let meta = SessionIdentityMeta::owner("cli");
    assert_eq!(meta.role, Role::Owner);
    assert_eq!(meta.identity_id, "owner");
    assert!(meta.scope.is_none());
    assert_eq!(meta.source_channel, "cli");
}

#[test]
fn test_session_identity_meta_guest_factory() {
    let scope = GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(2000),
        display_name: Some("Test Guest".to_string()),
    };

    let meta = SessionIdentityMeta::guest("guest-123", scope.clone(), "telegram");
    assert_eq!(meta.role, Role::Guest);
    assert_eq!(meta.identity_id, "guest-123");
    assert_eq!(meta.scope, Some(scope));
    assert_eq!(meta.source_channel, "telegram");
}

#[test]
fn test_session_identity_meta_json_roundtrip() {
    let scope = GuestScope {
        allowed_tools: vec!["tool1".to_string(), "tool2".to_string()],
        expires_at: None,
        display_name: None,
    };

    let meta = SessionIdentityMeta::guest("guest-456", scope, "web");
    let json = meta.to_json_string().unwrap();
    let parsed = SessionIdentityMeta::from_json_str(Some(&json));

    assert_eq!(parsed.role, meta.role);
    assert_eq!(parsed.identity_id, meta.identity_id);
    assert_eq!(parsed.scope, meta.scope);
    assert_eq!(parsed.source_channel, meta.source_channel);
}

#[test]
fn test_session_identity_meta_from_null_json() {
    let meta = SessionIdentityMeta::from_json_str(None);
    assert_eq!(meta.role, Role::Owner); // Default
    assert_eq!(meta.identity_id, "owner");
}

#[test]
fn test_session_identity_meta_from_invalid_json() {
    let meta = SessionIdentityMeta::from_json_str(Some("{invalid json}"));
    assert_eq!(meta.role, Role::Owner); // Fallback to default
}

#[tokio::test]
async fn test_tool_fields_persist_through_append_message_and_get_history() {
    use crate::gateway::session_store::types::MessageRecord;
    use crate::gateway::session_store::SessionStore;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Write a tool-result message through the SessionStore trait method — the
    // exact forwarding path (append_message → add_message_full) whose
    // correctness matters: it must forward tool_call_id/tool_name, not None.
    let msg = MessageRecord {
        id: "1".into(),
        role: "tool".into(),
        content: r#"{"result":"ok"}"#.into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: Some("call_abc123".into()),
        tool_name: Some("bash_exec".into()),
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, msg)
        .await
        .unwrap();

    let history = manager.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 1);
    let msg: &MessageRecord = &history[0];
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("call_abc123"));
    assert_eq!(msg.tool_name.as_deref(), Some("bash_exec"));
}

#[tokio::test]
async fn test_close_session_with_topic() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "Hello").await.unwrap();

    manager
        .close_session(&key, Some("测试对话".to_string()))
        .await
        .unwrap();

    // Verify topic can be retrieved
    let topic = manager.get_session_topic(&key).await.unwrap();
    assert_eq!(topic, Some("测试对话".to_string()));
}

#[tokio::test]
async fn legacy_session_history_readable_without_events() {
    use crate::gateway::session_store::types::MessageRecord;
    use crate::gateway::session_store::SessionStore;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Write two messages directly (legacy session — no session_events).
    // This simulates a session created before the MessageProjector flip.
    let user_msg = MessageRecord {
        id: "msg_1".into(),
        role: "user".into(),
        content: "Hello, assistant!".into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: None,
        tool_name: None,
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, user_msg)
        .await
        .unwrap();

    let assistant_msg = MessageRecord {
        id: "msg_2".into(),
        role: "assistant".into(),
        content: "Hi there!".into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: None,
        tool_name: None,
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, assistant_msg)
        .await
        .unwrap();

    // Assert both messages are readable through the read surface (messages table).
    let history = <SessionManager as SessionStore>::get_history(&manager, &key, None)
        .await
        .unwrap();
    assert_eq!(
        history.len(),
        2,
        "legacy session should return both messages"
    );
    assert_eq!(history[0].role, "user", "first message should be user role");
    assert_eq!(
        history[0].content, "Hello, assistant!",
        "first message content mismatch"
    );
    assert_eq!(
        history[1].role, "assistant",
        "second message should be assistant role"
    );
    assert_eq!(
        history[1].content, "Hi there!",
        "second message content mismatch"
    );
}

#[tokio::test]
async fn test_close_session_without_topic() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    manager.close_session(&key, None).await.unwrap();

    let topic = manager.get_session_topic(&key).await.unwrap();
    assert!(topic.is_none());
}

#[tokio::test]
async fn test_get_current_epoch() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Create epoch 0
    let key0 = SessionKey::main("test");
    manager.get_or_create(&key0).await.unwrap();
    let epoch = manager.get_current_epoch("agent:test:main").await.unwrap();
    assert_eq!(epoch, 0);
}

/// The two failures the previous `LIKE '<base>%' ORDER BY created_at DESC
/// LIMIT 1` had, both of which the FILE backend already got right:
///
/// 1. no separator anchor, so a longer peer id was a string-prefix match and
///    lent its epoch to the shorter one — routing that peer's every inbound
///    message into a session it had never spoken in;
/// 2. newest-CREATED, not highest-epoch.
#[tokio::test]
async fn get_current_epoch_is_max_and_never_borrows_a_prefix_sibling() {
    use crate::routing::session_key::DmScope;
    let temp = tempdir().unwrap();
    let manager = SessionManager::new(test_config(temp.path().join("epoch.db"))).unwrap();

    let short = SessionKey::dm("test", "telegram", "123", DmScope::PerPeer);
    let long = SessionKey::dm("test", "telegram", "1234", DmScope::PerPeer);
    // ONLY the sibling exists. Peer 123 has never spoken, so the honest answer
    // is 0 — and this is the shape that pins the separator anchor
    // deterministically: seeding peer 123's own row too would leave the old
    // `ORDER BY created_at DESC LIMIT 1` picking between two rows that share a
    // whole-second `created_at`, so the wrong implementation passed about half
    // the time.
    manager.get_or_create(&long.with_epoch(2)).await.unwrap();

    assert_eq!(
        manager
            .get_current_epoch(&short.base_key_pattern())
            .await
            .unwrap(),
        0,
        "peer 123 borrowed peer 1234's epoch — every message it sends is \
         routed into a blank session and its real conversation is unreachable"
    );

    // Highest wins regardless of insertion order.
    let base = SessionKey::main("epochmax");
    for e in [0u32, 3, 1] {
        manager.get_or_create(&base.with_epoch(e)).await.unwrap();
    }
    assert_eq!(
        manager
            .get_current_epoch(&base.base_key_pattern())
            .await
            .unwrap(),
        3,
        "newest-created is not highest-epoch"
    );
}

/// `last_message_preview` had a writer only in the FILE backend while two
/// shipped readers (`sessions.preview`, the `sessions` tool row) surfaced it
/// regardless of backend — so a `session_store_backend = "sqlite"` install
/// showed a null preview for every conversation, forever.
#[tokio::test]
async fn sqlite_maintains_the_last_message_preview_the_file_backend_does() {
    let temp = tempdir().unwrap();
    let manager = SessionManager::new(test_config(temp.path().join("preview.db"))).unwrap();
    let key = SessionKey::main("previewtest");
    manager.get_or_create(&key).await.unwrap();
    manager
        .add_message(&key, "user", "  where is the deploy log?  ")
        .await
        .unwrap();

    let meta = crate::gateway::session_store::SessionStore::get_metadata(&manager, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        meta.last_message_preview.as_deref(),
        Some("where is the deploy log?"),
        "the preview column has no writer on this backend"
    );

    manager
        .add_message(&key, "assistant", "in ~/.aleph/logs")
        .await
        .unwrap();
    let meta = crate::gateway::session_store::SessionStore::get_metadata(&manager, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        meta.last_message_preview.as_deref(),
        Some("in ~/.aleph/logs"),
        "the preview must follow the LAST message, not the first"
    );
}

#[test]
fn test_session_identity_meta_to_identity_context_owner() {
    let meta = SessionIdentityMeta::owner("cli");
    let ctx = meta.to_identity_context("session:main".to_string());

    assert_eq!(ctx.session_key, "session:main");
    assert_eq!(ctx.role, Role::Owner);
    assert_eq!(ctx.identity_id, "owner");
    assert_eq!(ctx.source_channel, "cli");
    assert!(ctx.scope.is_none());
}

#[test]
fn test_session_identity_meta_to_identity_context_guest() {
    let scope = GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(3000),
        display_name: Some("Guest".to_string()),
    };

    let meta = SessionIdentityMeta::guest("guest-789", scope.clone(), "telegram");
    let ctx = meta.to_identity_context("session:guest".to_string());

    assert_eq!(ctx.session_key, "session:guest");
    assert_eq!(ctx.role, Role::Guest);
    assert_eq!(ctx.identity_id, "guest-789");
    assert_eq!(ctx.source_channel, "telegram");
    assert_eq!(ctx.scope, Some(scope));
}

#[tokio::test]
async fn test_get_total_tokens_none_then_accumulates() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();
    let key = SessionKey::main("tok");

    // No row yet → None.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), None);

    manager.get_or_create(&key).await.unwrap();
    // Fresh row → 0.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), Some(0));

    manager
        .update_session_usage(&key, 100, 40, 0.25, None, None)
        .await
        .unwrap();
    manager
        .update_session_usage(&key, 10, 5, 0.05, None, None)
        .await
        .unwrap();
    // Cumulative input+output across both turns: 140 + 15 = 155.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), Some(155));

    // …and the cost accumulates on the same row. `estimated_cost_usd` had no
    // column and no writer until now, yet the `sessions` tool and the Panel both
    // reported it to the user as this session's spend — permanently $0.00.
    let sessions = manager.list_sessions(None).await.unwrap();
    let meta = sessions
        .iter()
        .find(|m| m.key == key.to_key_string())
        .expect("session row");
    assert!(
        (meta.estimated_cost_usd - 0.30).abs() < 1e-9,
        "expected 0.25 + 0.05 = 0.30, got {}",
        meta.estimated_cost_usd
    );
}

#[tokio::test]
async fn goal_tree_budget_sums_own_plus_member_deltas_only() {
    use crate::gateway::goal_budget::tree_tokens;
    use crate::gateway::session_store::SessionStore;
    use crate::goal::types::{BudgetMember, Goal};
    use std::sync::Arc;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Owner session: seed 100+40 = 140 cumulative tokens.
    let own_key = SessionKey::main("leader");
    manager.get_or_create(&own_key).await.unwrap();
    manager
        .update_session_usage(&own_key, 100, 40, 0.0, None, None)
        .await
        .unwrap();

    // Enrolled member (a delegated task session): seed 200+60 = 260; joined at 60.
    let member_key = SessionKey::task("worker", "team", "task-1");
    manager.get_or_create(&member_key).await.unwrap();
    manager
        .update_session_usage(&member_key, 200, 60, 0.0, None, None)
        .await
        .unwrap();

    // An UNENROLLED session (stands in for an in-process subagent — never a budget member).
    let stray_key = SessionKey::task("worker", "team", "stray");
    manager.get_or_create(&stray_key).await.unwrap();
    manager
        .update_session_usage(&stray_key, 9_999, 9_999, 0.0, None, None)
        .await
        .unwrap();

    // Goal owned by own_key; one enrolled member with tokens_at_join = 60.
    let mut goal = Goal::new(&own_key.to_key_string(), "obj", 0, 0);
    goal.token_budget = Some(10_000);
    goal.budget_members = vec![BudgetMember {
        session_id: member_key.to_key_string(),
        tokens_at_join: 60,
    }];

    let store: Arc<dyn SessionStore> = Arc::new(manager);
    let total = tree_tokens(&store, &goal, &own_key)
        .await
        .expect("own total readable");

    // own(140) + member_delta(260 - 60 = 200) = 340. The stray 19_998 is absent.
    assert_eq!(
        total, 340,
        "only own row + enrolled member delta count; unenrolled spend is invisible"
    );
}

/// The SQLite half of the idle sweep's existence floor. The file half — and the
/// reasoning both share — is in
/// `session_store::file_backend::reap_tests::cleanup_expired_measures_idleness_from_when_the_session_existed`
/// and on [`SessionStore::cleanup_expired`].
///
/// This half is the newer hazard of the two: SQLite's `last_active_at` was
/// `Utc::now()` at insert until `add_message_full` began following the message,
/// so before that a conversation could not arrive already-expired no matter
/// what its stamps said. The file backend has taken the message clock since it
/// was written.
///
/// [`SessionStore::cleanup_expired`]: crate::gateway::session_store::SessionStore::cleanup_expired
#[cfg(test)]
mod idle_sweep_floor_tests {
    use super::*;
    use crate::gateway::session_store::SessionStore;
    use rusqlite::params;

    const DAY: i64 = 86_400;

    /// Set both clocks directly. `last_active_at` alone would be reachable
    /// through `append_message`, but `created_at` has exactly one writer —
    /// session creation — which is what makes it usable as a floor.
    fn set_clocks(manager: &SessionManager, key: &SessionKey, idle_secs: i64, existed_secs: i64) {
        let now = chrono::Utc::now().timestamp();
        manager
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE sessions SET last_active_at = ?1, created_at = ?2 WHERE key = ?3",
                params![now - idle_secs, now - existed_secs, key.to_key_string()],
            )
            .unwrap();
    }

    async fn exists(manager: &SessionManager, key: &SessionKey) -> bool {
        <SessionManager as SessionStore>::get_metadata(manager, key)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn cleanup_expired_measures_idleness_from_when_the_session_existed() {
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("test.db"),
            session_expiry_secs: (30 * DAY) as u64,
            ..Default::default()
        })
        .unwrap();

        let aged = SessionKey::ephemeral("aged");
        let replayed = SessionKey::ephemeral("replayed");
        manager.get_or_create(&aged).await.unwrap();
        manager.get_or_create(&replayed).await.unwrap();
        // Quiet for 100 days and around for 100 days.
        set_clocks(&manager, &aged, 100 * DAY, 100 * DAY);
        // A conversation from 100 days ago, projected into this row moments
        // ago: an import, a backfill, a reconciler replaying an old event.
        set_clocks(&manager, &replayed, 100 * DAY, 5);

        let deleted = <SessionManager as SessionStore>::cleanup_expired(&manager)
            .await
            .unwrap();

        assert_eq!(
            deleted, 1,
            "expected exactly the session that has been idle as long as it has \
             existed"
        );
        assert!(
            !exists(&manager, &aged).await,
            "the genuinely idle session survived — the floor disabled the sweep \
             instead of bounding it"
        );
        assert!(
            exists(&manager, &replayed).await,
            "a session created five seconds ago was swept as expired, because \
             the conversation it records happened 100 days ago"
        );
    }
}
