//! `core/session-log` — name the contradictions a session's event log holds.
//!
//! Two production sentences promised this check before it existed. `aleph
//! resume` answers `log_inconsistent` with *"Run `aleph doctor` — the
//! `core/session-log` check names the contradiction"*, and
//! [`crate::gateway::ResumeReport::contradictions`] documents itself as *"a
//! magnitude for the boot line — the kinds themselves are named per session by
//! the `core/session-log` doctor check"*. Both were shipped in T2/T3 of the
//! crash-recovery round; the check was not. An operator following that sentence
//! reached `aleph doctor` and found no such check, which is worse than the
//! magnitude alone: a label that names a thing the reader cannot find reads as
//! their mistake, not as a missing feature.
//!
//! The reduction is not repeated here. [`reduce_run`] is the one derivation,
//! and this check only renders what it returns: `Err` is a REJECT kind (the
//! reducer refused the log and nothing downstream may read it as clean), and
//! `Ok(_).contradictions` is every REPORT kind, which the reducer worked around
//! with a stated reading. Each is named by its own
//! [`LogContradiction::tag`] — the tags are already spelled `session-log-*`,
//! because they were written for this surface.
//!
//! The rows are read one at a time (`load_rows`), so a row this build cannot
//! decode is a value here rather than the read's failure: the session is
//! named as refused under [`LogContradiction::UndecodableRecord`] with the
//! row's seq and `type`, and it is not reduced — the reducer never saw the
//! record, so no reading of the rest exists. A row a newer build marked
//! `ignorable` is skipped and counted.
//!
//! **Contradictions are report-only, deliberately.** A log whose markers
//! contradict each other cannot be mechanically resolved without deciding
//! which of two disagreeing records is the truth, and that decision belongs
//! to whoever reads the transcript. The ONE repair this check owns is the
//! undecodable record: there is nothing to decide between — the build cannot
//! read the row at all — and the only mechanical exit is to take exactly that
//! row out of the live log, which `fix=true` does through
//! [`SessionEventStore::retire_record`], one record at a time, nothing else.
//! The neighbouring `core/projection-holes` repairs more freely because a
//! missing projection row has exactly one correct value — the event it was
//! derived from.
//!
//! Registered only via
//! [`crate::diagnostics::DiagnosticEngine::with_session_log_check`], for the
//! same reason as its sibling: it needs the open event log, which the cold
//! `aleph-server doctor` process does not have. A missing handle reports
//! UNKNOWN — never "no contradictions", which is the one answer this check may
//! not fabricate.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};
use crate::gateway::session_projector::MessageProjector;
use crate::gateway::session_store::types::SessionFilter;
use crate::session::events::SessionEventRecord;
use crate::session::reduction::{reduce_run, LogContradiction};
use crate::session::service::SessionId;
use crate::session::store::{DecodedRow, SessionEventStore, UndecodableRecord};
use crate::sync_primitives::Arc;

const ID: &str = "core/session-log";
/// Noun phrase the "unknown" finding is titled with — `"Session log unknown"`.
const SUBJECT: &str = "Session log";

/// How many plain contradicting sessions to name in the detail. The counts
/// stay exact; only that roll-call is capped. A session with an undecodable
/// row is never subject to it — it is part of the repair set, and the repair
/// hint promises to have named every one.
const NAMED_LIMIT: usize = 10;

pub struct SessionLogCheck {
    projector: Option<Arc<MessageProjector>>,
    event_store: Option<Arc<dyn SessionEventStore>>,
}

impl SessionLogCheck {
    /// `projector` is here only to enumerate sessions — its projection store is
    /// the one thing in reach that lists sessions with no run markers at all,
    /// and [`crate::session::reduction::LogContradiction::UnmarkedActivity`] is
    /// exactly the kind such a session holds. Reading the logs is the event
    /// store's job and nothing is written through either handle.
    ///
    /// `None` for either is honest: the check then says it could not look.
    #[must_use]
    pub const fn new(
        projector: Option<Arc<MessageProjector>>,
        event_store: Option<Arc<dyn SessionEventStore>>,
    ) -> Self {
        Self {
            projector,
            event_store,
        }
    }
}

/// One session's verdict.
struct Contradicting {
    key: String,
    /// `true` when the log was REFUSED — by the reducer (`Err`), or by the
    /// decoder before the reducer ever saw it — rather than read with a stated
    /// correction. The two are different answers to the operator: a refused
    /// log is why a resume did nothing.
    refused: bool,
    tags: Vec<&'static str>,
    /// The rows this build could not decode, in `seq` order. Non-empty only on
    /// a refused entry, and the one thing `fix=true` retires.
    undecodable: Vec<UndecodableRecord>,
}

/// What one session's rows say, once walked.
enum RowsVerdict {
    /// Every row decoded (ignorable ones skipped): reduce these.
    Decoded(Vec<SessionEventRecord>),
    /// At least one row did not decode: refused, and these are named.
    Undecodable(Vec<UndecodableRecord>),
}

/// Walk one session's rows: every undecodable one is collected (the fix
/// retires them all), skipped ones are counted, and the decoded events are
/// handed on only when nothing was undecodable.
fn walk_rows(rows: Vec<DecodedRow>, skipped: &mut usize) -> RowsVerdict {
    let mut events = Vec::with_capacity(rows.len());
    let mut undecodable = Vec::new();
    for row in rows {
        match row {
            DecodedRow::Event(r) => events.push(r),
            DecodedRow::Skipped { .. } => *skipped += 1,
            DecodedRow::Undecodable(u) => undecodable.push(u),
        }
    }
    if undecodable.is_empty() {
        RowsVerdict::Decoded(events)
    } else {
        RowsVerdict::Undecodable(undecodable)
    }
}

/// One roll-call entry: `key [refused: tag, tag (seq N type `T`, …)]`. An
/// undecodable entry names each row by seq and `type`, so the operator can
/// find it in the transcript before retiring it.
fn name_entry(b: &Contradicting) -> String {
    let rows: String = b
        .undecodable
        .iter()
        .map(|u| {
            format!(
                "seq {} type `{}`",
                u.seq,
                u.kind_tag.as_deref().unwrap_or("?")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} [{}{}{}]",
        b.key,
        if b.refused { "refused: " } else { "" },
        b.tags.join(", "),
        if rows.is_empty() {
            String::new()
        } else {
            format!(" ({rows})")
        }
    )
}

/// `; N ignorable row(s) skipped` when any were, for both the OK finding and
/// the problem finding — a skipped row is a fact about the log either way.
fn skipped_note(skipped: usize) -> String {
    if skipped > 0 {
        format!("; {skipped} ignorable row(s) skipped")
    } else {
        String::new()
    }
}

#[async_trait]
impl HealthCheck for SessionLogCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Session log"
    }

    async fn run(&self, posture: Posture) -> Vec<Finding> {
        let Some(events) = self.event_store.as_ref() else {
            return vec![unknown_finding(
                ID,
                SUBJECT,
                "this doctor run has no open session event log, so no session's run \
                 markers could be read at all. Run `aleph doctor` against the running \
                 daemon rather than `aleph-server doctor`, which is a cold process.",
            )];
        };

        // The candidate set is a UNION on purpose. `load_run_markers` omits
        // sessions with no markers, and a session with tool activity and no
        // `RunStarted` is precisely `UnmarkedActivity` — enumerating from the
        // markers alone would make this check structurally blind to one of the
        // kinds it exists to name.
        let mut keys: BTreeMap<String, ()> = BTreeMap::new();
        let mut list_unreadable: Option<String> = None;
        match events.load_run_markers().await {
            Ok(rows) => {
                for (id, _) in rows {
                    keys.insert(id.to_key_string(), ());
                }
            }
            Err(e) => list_unreadable = Some(e.to_string()),
        }
        if let Some(projector) = self.projector.as_ref() {
            if let Ok(metas) = projector
                .projection_store()
                .list_sessions(SessionFilter::default())
                .await
            {
                for meta in metas {
                    keys.insert(meta.key, ());
                }
            }
        }

        if keys.is_empty() {
            return match list_unreadable {
                Some(e) => vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!(
                        "the run markers could not be read ({e}), so no session's log was reduced."
                    ),
                )],
                None => vec![Finding::ok(
                    ID,
                    "Session logs consistent",
                    "no session has an event log yet.",
                )],
            };
        }

        let mut bad: Vec<Contradicting> = Vec::new();
        let mut unreadable: usize = 0;
        let mut skipped: usize = 0;
        for key in keys.keys() {
            let Some(id) = SessionId::from_key_string(key) else {
                unreadable += 1;
                continue;
            };
            let Ok(rows) = events.load_rows(&id).await else {
                unreadable += 1;
                continue;
            };
            let log = match walk_rows(rows, &mut skipped) {
                RowsVerdict::Decoded(log) => log,
                RowsVerdict::Undecodable(undecodable) => {
                    let mut tags: Vec<&'static str> = undecodable
                        .iter()
                        .map(|u| LogContradiction::from(u).tag())
                        .collect();
                    tags.dedup();
                    bad.push(Contradicting {
                        key: key.clone(),
                        refused: true,
                        tags,
                        undecodable,
                    });
                    continue;
                }
            };
            match reduce_run(&log) {
                Err(c) => bad.push(Contradicting {
                    key: key.clone(),
                    refused: true,
                    tags: vec![c.tag()],
                    undecodable: Vec::new(),
                }),
                Ok(r) if !r.contradictions.is_empty() => {
                    let mut tags: Vec<&'static str> =
                        r.contradictions.iter().map(LogContradiction::tag).collect();
                    tags.dedup();
                    bad.push(Contradicting {
                        key: key.clone(),
                        refused: false,
                        tags,
                        undecodable: Vec::new(),
                    });
                }
                Ok(_) => {}
            }
        }

        let scanned = keys.len();
        if bad.is_empty() && unreadable == 0 && list_unreadable.is_none() {
            return vec![Finding::ok(
                ID,
                "Session logs consistent",
                format!(
                    "{scanned} session log(s) reduced; none contradicts itself{}.",
                    skipped_note(skipped)
                ),
            )];
        }
        if bad.is_empty() {
            // Nothing measured as contradicting, but something could not be
            // measured. That is not a pass.
            return vec![unknown_finding(
                ID,
                SUBJECT,
                format!(
                    "{unreadable} of {scanned} session log(s) could not be reduced \
                     (unparseable key, or the log would not read){}. The rest are \
                     consistent.",
                    list_unreadable
                        .as_deref()
                        .map_or_else(String::new, |e| format!(
                            ", and the marker scan failed ({e})"
                        ))
                ),
            )];
        }

        let refused = bad.iter().filter(|b| b.refused).count();
        // The roll-call: every entry with an undecodable row first and
        // UNCAPPED — they are exactly the set `fix=true` touches, and a hint
        // that says "the records named above" must be able to mean it — then
        // the plain contradicting sessions, capped.
        let (with_undecodable, plain): (Vec<&Contradicting>, Vec<&Contradicting>) =
            bad.iter().partition(|b| !b.undecodable.is_empty());
        let named: Vec<String> = with_undecodable
            .iter()
            .chain(plain.iter().take(NAMED_LIMIT))
            .map(|b| name_entry(b))
            .chain(
                (plain.len() > NAMED_LIMIT)
                    .then(|| format!("… and {} more", plain.len() - NAMED_LIMIT)),
            )
            .collect();
        let undecodable_total: usize = bad.iter().map(|b| b.undecodable.len()).sum();

        let mut finding = Finding::problem(
            ID,
            // A refused log is the one that already cost the operator a resume;
            // a reported one was read, with a correction the reducer states.
            if refused > 0 {
                Severity::Error
            } else {
                Severity::Warning
            },
            "Session logs contradict themselves",
            format!(
                "{} of {scanned} session log(s) contradict themselves{}: {}{}.{}",
                bad.len(),
                if refused > 0 {
                    format!(", {refused} of them refused")
                } else {
                    String::new()
                },
                named.join("; "),
                skipped_note(skipped),
                if unreadable > 0 {
                    format!(" A further {unreadable} could not be reduced.")
                } else {
                    String::new()
                }
            ),
        );
        if undecodable_total == 0 {
            return vec![finding.with_fix_hint(
                "Not mechanically repairable: resolving a contradiction means deciding which \
                 of two disagreeing records is true, which this check cannot do for you. A \
                 `refused` log is why `aleph resume` answered `log_inconsistent` for that \
                 session — read the named seq in the transcript. A reported kind was worked \
                 around with a stated reading and costs nothing but the note.",
            )];
        }

        finding = finding
            .with_fix_hint(format!(
                "`fix=true` retires ONLY the {undecodable_total} undecodable record(s) named \
                 above (every one is listed; the cap applies to the other sessions) — rows \
                 this build cannot read at all, so there is nothing to decide between; the \
                 rest of each log stays live. Contradictions stay report-only: resolving one \
                 means deciding which of two disagreeing records is true, which this check \
                 cannot do for you."
            ))
            .repairable();
        if posture.allows_repair() {
            finding = finding.with_repair(retire_undecodable(events.as_ref(), &bad).await);
        }
        vec![finding]
    }
}

/// The repair: retire every undecodable record named on the finding, one
/// `retire_record` each, and report exactly what happened to each — retired,
/// already gone (a concurrent retire), or refused by the store.
async fn retire_undecodable(
    events: &dyn SessionEventStore,
    bad: &[Contradicting],
) -> RepairOutcome {
    let mut retired = 0usize;
    let mut already = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for b in bad {
        let Some(id) = SessionId::from_key_string(&b.key) else {
            // Unreachable for an entry that was read through `load_rows`
            // above; said out loud rather than skipped in silence.
            failed.push(format!("{}: key no longer parses", b.key));
            continue;
        };
        for u in &b.undecodable {
            match events.retire_record(&id, u.seq).await {
                Ok(true) => retired += 1,
                Ok(false) => already += 1,
                Err(e) => failed.push(format!("{} seq {}: {e}", b.key, u.seq)),
            }
        }
    }
    if failed.is_empty() {
        RepairOutcome::Repaired {
            detail: format!(
                "Retired {retired} undecodable record(s){}",
                if already > 0 {
                    format!("; {already} already retired")
                } else {
                    String::new()
                }
            ),
        }
    } else {
        RepairOutcome::Failed {
            error: format!(
                "{retired} record(s) retired; {} could not be: {}",
                failed.len(),
                failed.join("; ")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::SessionEvent;
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    /// The one answer this check may never give without looking. `None` for the
    /// event store is the cold-process shape (`aleph-server doctor`), and the
    /// sentence the CLI prints sends the operator here — landing on "consistent"
    /// would confirm a log nobody read.
    #[tokio::test]
    async fn a_check_with_no_event_log_says_unknown_not_consistent() {
        let check = SessionLogCheck::new(None, None);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check_id, ID);
        assert!(
            findings[0].title.contains("unknown"),
            "expected an unknown finding, got: {}",
            findings[0].title
        );
        assert!(
            !findings[0].detail.contains("consistent"),
            "an unlooked-at log must not be described as consistent: {}",
            findings[0].detail
        );
    }

    /// The tags this check renders are the reducer's, not a second vocabulary
    /// spelled here. Goes red the day a `LogContradiction` variant is added
    /// whose tag stops being `session-log-`-prefixed, which is the moment the
    /// check's detail line would start naming something the operator cannot
    /// grep for. Walks the reducer's own one-per-kind list rather than a
    /// hand-picked few, so every kind — including the next one — is asked.
    #[test]
    fn every_contradiction_tag_belongs_to_this_checks_namespace() {
        let all = crate::session::reduction::fixtures::one_of_each_kind();
        assert_eq!(
            all.len(),
            crate::session::reduction::fixtures::KIND_COUNT,
            "the census must see the whole closed set"
        );
        for c in all {
            assert!(
                c.tag().starts_with("session-log-"),
                "tag {} is outside this check's namespace",
                c.tag()
            );
        }
    }

    /// An in-memory event log holding one session: `RunStarted` at seq 1 and,
    /// at `bad_seq`, a row written by a build this one does not know — inserted
    /// raw, past `encode_row`, exactly as it would sit on disk.
    async fn seeded_store_with_bad_row(bad_seq: i64) -> (Arc<SqliteEventStore>, SessionId) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store = Arc::new(SqliteEventStore::new(conn));
        let sid = SessionKey::main("doctor-undecodable");
        store
            .append(
                &sid,
                1,
                &SessionEvent::RunStarted {
                    run_id: "r".into(),
                    at: 1,
                    project_root: None,
                    envelope: None,
                },
                1,
            )
            .await
            .unwrap();
        store
            .insert_raw_row_for_test(
                &sid,
                bad_seq,
                "from_the_future",
                r#"{"type":"from_the_future","v":9}"#,
            )
            .await;
        (store, sid)
    }

    /// The one mechanical repair this check owns: a row this build cannot read
    /// is named by seq and kind on the finding, the finding is repairable, and
    /// `fix=true` retires exactly that record — the decodable rows around it
    /// stay live.
    #[tokio::test]
    async fn an_undecodable_row_is_named_and_fix_retires_exactly_that_record() {
        let (store, sid) = seeded_store_with_bad_row(7).await;
        let check = SessionLogCheck::new(None, Some(store.clone() as Arc<dyn SessionEventStore>));
        let f = &check.run(Posture::Inspect).await[0];
        assert!(
            f.detail.contains("session-log-undecodable-record") && f.detail.contains("seq 7"),
            "{}",
            f.detail
        );
        assert!(f.repairable);
        assert!(
            f.repair_outcome.is_none(),
            "Inspect must not have retired anything"
        );
        assert!(
            store.load_all_events(&sid).await.is_err(),
            "the premise: the log is unreadable before the fix"
        );
        let f = &check.run(Posture::Fix).await[0];
        assert!(
            matches!(f.repair_outcome, Some(RepairOutcome::Repaired { .. })),
            "{:?}",
            f.repair_outcome
        );
        assert_eq!(
            store.load_all_events(&sid).await.unwrap().len(),
            1,
            "only the bad record was retired"
        );
        // Retired, the row is out of the live log: the next run reads clean.
        let f = &check.run(Posture::Inspect).await[0];
        assert!(!f.is_problem(), "{}", f.detail);
    }

    /// The hint says `fix=true` retires the undecodable records "named
    /// above", so they must be named whatever the roll-call cap does: eleven
    /// plain contradicting sessions (each a `RunFinished` closing no run —
    /// `FinishWithoutStart`) sort BEFORE one undecodable session, which is
    /// therefore past `NAMED_LIMIT` — and is still named, with its seq, and
    /// counted in the hint.
    #[tokio::test]
    async fn an_undecodable_entry_is_named_past_the_roll_call_cap_and_counted() {
        let (store, sid) = seeded_store_with_bad_row(7).await;
        assert!(
            SessionKey::main("a-plain-00").to_key_string() < sid.to_key_string(),
            "the premise: the plain keys sort before the undecodable one"
        );
        for i in 0..=NAMED_LIMIT {
            store
                .append(
                    &SessionKey::main(format!("a-plain-{i:02}")),
                    1,
                    &SessionEvent::RunFinished {
                        run_id: "orphan".into(),
                        outcome: crate::session::events::RunOutcome::Completed,
                        at: 1,
                    },
                    1,
                )
                .await
                .unwrap();
        }
        let check = SessionLogCheck::new(None, Some(store.clone() as Arc<dyn SessionEventStore>));
        let f = &check.run(Posture::Inspect).await[0];
        assert!(f.repairable, "{}", f.detail);
        assert!(
            f.detail.contains(&sid.to_key_string()) && f.detail.contains("seq 7"),
            "the undecodable session is named past the cap: {}",
            f.detail
        );
        assert!(
            f.detail.contains("… and 1 more"),
            "the cap still applies to the plain sessions: {}",
            f.detail
        );
        let hint = f.fix_hint.as_deref().unwrap_or_default();
        assert!(
            hint.contains("the 1 undecodable record(s) named above"),
            "the hint carries the count of what the repair touches: {hint}"
        );
    }
}
