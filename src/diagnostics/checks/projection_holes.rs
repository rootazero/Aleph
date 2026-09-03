//! `core/projection-holes` — the unbounded sweep for transcript rows the
//! `messages` projection is missing.
//!
//! `session_events` is the SSOT; `messages` is an asynchronously-drained read
//! projection. A crash between an event's durable append and its drain leaves a
//! gap, and the in-process record of which seqs went missing dies with the
//! process. Boot repairs the **activity window**
//! ([`crate::gateway::projection_reconciler`]); everything older is this
//! check's job — it is the only place in the system that walks EVERY session,
//! which is exactly why it is a doctor check and not a boot pass.
//!
//! Detection is a seq-set difference, the same predicate the projector's heal
//! uses: the projected row ids carry their source seq
//! ([`crate::session::projection::parse_source_seq`]), so a hole below the
//! newest row is as visible as a missing tail.
//!
//! The repair is `MessageProjector::request_repair`, i.e. the projector's own
//! drain task — never a write from here. The drain is the single writer for a
//! session; repairing from a diagnostic thread would be a second one.
//!
//! Registered only via [`crate::diagnostics::DiagnosticEngine::with_projection_holes_check`]
//! and deliberately absent from `default_registry()`: it needs the live
//! projector and the open event log, neither of which exists in the cold
//! `aleph-server doctor` process. A missing handle reports UNKNOWN rather than
//! "no holes" — see the arm below.

use std::collections::HashSet;

use async_trait::async_trait;

use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, RepairOutcome, Severity};
use crate::gateway::session_projector::MessageProjector;
use crate::gateway::session_store::types::SessionFilter;
use crate::session::projection::parse_source_seq;
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

const ID: &str = "core/projection-holes";
/// Noun phrase the "unknown" finding is titled with — `"Transcript projection
/// unknown"`. See [`crate::diagnostics::check::unknown_finding`].
const SUBJECT: &str = "Transcript projection";

/// How many holed sessions to name in the finding's detail. The count is
/// always exact; only the roll-call is capped, because an operator reading a
/// hundred session keys learns nothing the number did not already say.
const NAMED_LIMIT: usize = 10;

pub struct ProjectionHolesCheck {
    projector: Option<Arc<MessageProjector>>,
    event_store: Option<Arc<dyn SessionEventStore>>,
}

impl ProjectionHolesCheck {
    /// Both handles come from the daemon. `None` for either is honest: the
    /// check then says it could not look, which is the one thing it must never
    /// render as a clean transcript.
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

/// One session's gap count, or the reason it could not be measured.
struct Holed {
    key: String,
    missing: usize,
}

#[async_trait]
impl HealthCheck for ProjectionHolesCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Transcript projection"
    }

    async fn run(&self, posture: Posture) -> Vec<Finding> {
        let (Some(projector), Some(events)) = (self.projector.as_ref(), self.event_store.as_ref())
        else {
            return vec![unknown_finding(
                ID,
                SUBJECT,
                "this doctor run has no live projector and/or no open session event log, \
                 so the transcript projection could not be compared against the event \
                 log at all. Run `aleph doctor` against the running daemon rather than \
                 `aleph-server doctor`, which is a cold process.",
            )];
        };

        let store = projector.projection_store();
        // The only unbounded scan in the system, on purpose and in one place:
        // no `active_minutes`, no limit.
        let sessions = match store.list_sessions(SessionFilter::default()).await {
            Ok(s) => s,
            Err(e) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!(
                        "the session list could not be read ({e}), so no session's \
                         transcript could be compared against its event log."
                    ),
                )];
            }
        };

        let mut holed: Vec<Holed> = Vec::new();
        let mut unreadable: usize = 0;
        for meta in &sessions {
            let Some(id) = SessionId::from_key_string(&meta.key) else {
                unreadable += 1;
                continue;
            };
            match holes_in(&store, events, &id).await {
                Ok(0) => {}
                Ok(missing) => holed.push(Holed {
                    key: meta.key.clone(),
                    missing,
                }),
                Err(()) => unreadable += 1,
            }
        }

        let total: usize = holed.iter().map(|h| h.missing).sum();
        if holed.is_empty() && unreadable == 0 {
            return vec![Finding::ok(
                ID,
                "Transcript projection complete",
                format!(
                    "{} session(s) checked; every projectable event has its row.",
                    sessions.len()
                ),
            )];
        }
        if holed.is_empty() {
            // Nothing measured as holed, but some sessions could not be
            // measured — that is not a pass.
            return vec![unknown_finding(
                ID,
                SUBJECT,
                format!(
                    "{unreadable} of {} session(s) could not be compared against their \
                     event log (unparseable key, or the log or transcript would not \
                     read). The rest are complete.",
                    sessions.len()
                ),
            )];
        }

        let mut named: Vec<String> = holed
            .iter()
            .take(NAMED_LIMIT)
            .map(|h| format!("{} ({} row(s))", h.key, h.missing))
            .collect();
        if holed.len() > NAMED_LIMIT {
            named.push(format!("… and {} more", holed.len() - NAMED_LIMIT));
        }
        let mut finding = Finding::problem(
            ID,
            Severity::Warning,
            "Transcript rows missing from the projection",
            format!(
                "{total} event(s) across {} session(s) are durable in the event log but \
                 have no row in the transcript the Panel reads: {}.{}",
                holed.len(),
                named.join(", "),
                if unreadable > 0 {
                    format!(" A further {unreadable} session(s) could not be compared.")
                } else {
                    String::new()
                }
            ),
        )
        .with_fix_hint(
            "Run `aleph doctor --fix` — the repair replays the missing events through \
             the projector's own drain task, which is idempotent and cannot duplicate \
             rows that are already there.",
        )
        .repairable();

        if posture.allows_repair() {
            let mut filled = 0usize;
            let mut failed = 0usize;
            for h in &holed {
                let Some(id) = SessionId::from_key_string(&h.key) else {
                    failed += 1;
                    continue;
                };
                let report = projector.request_repair(&id).await;
                if report.errored {
                    failed += 1;
                } else {
                    filled += report.holes_filled;
                }
            }
            let outcome = if failed > 0 {
                RepairOutcome::Failed {
                    error: format!(
                        "{filled} row(s) filled; {failed} session(s) could not be repaired"
                    ),
                }
            } else {
                RepairOutcome::Repaired {
                    detail: format!("Filled {filled} missing transcript row(s)"),
                }
            };
            finding = finding.with_repair(outcome);
        }

        vec![finding]
    }
}

/// How many of this session's projectable events have no row.
///
/// `Err(())` means "I could not compare", never "zero": the caller counts it
/// separately and the check reports UNKNOWN rather than a pass.
async fn holes_in(
    store: &Arc<dyn crate::gateway::session_store::SessionStore>,
    events: &Arc<dyn SessionEventStore>,
    id: &SessionId,
) -> Result<usize, ()> {
    let key = id.to_key_string();
    let transcript = store.get_history(id, None).await.map_err(|_| ())?;
    let present: HashSet<u64> = transcript
        .iter()
        .filter_map(|m| parse_source_seq(&m.id, &key))
        .collect();
    // A non-empty transcript with no projector seq ids is foreign / pre-SSOT
    // content. It is not a hole and must not be counted as one — the projector
    // refuses to touch it for the same reason.
    if !transcript.is_empty() && present.is_empty() {
        return Ok(0);
    }
    let log = events.load_all_events(id).await.map_err(|_| ())?;
    Ok(log
        .iter()
        .filter(|rec| {
            crate::session::projection::project_row(&rec.event).is_some()
                && !present.contains(&rec.seq)
        })
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use crate::session::events::{MessageContent, SessionEvent};
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    fn fixture() -> (
        Arc<dyn SessionStore>,
        Arc<dyn SessionEventStore>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: dir.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        );
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        (store, Arc::new(SqliteEventStore::new(conn)), dir)
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 0,
            synthetic: false,
            author_user_id: None,
        }
    }

    /// The handles are what let this check answer at all. Without them it must
    /// say so — a check that renders "no holes" from "I could not look" is the
    /// reassuring line in front of a transcript with a gap in it.
    #[tokio::test]
    async fn a_cold_process_reports_unknown_not_clean() {
        let findings = ProjectionHolesCheck::new(None, None)
            .run(Posture::Inspect)
            .await;
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].title.contains("unknown"),
            "got {:?}",
            findings[0].title
        );
        assert!(findings[0].is_problem() || findings[0].severity == Severity::Warning);
    }

    /// Detection and repair, end to end: a session whose events are all in the
    /// log and whose transcript is empty.
    #[tokio::test]
    async fn a_holed_session_is_found_and_repaired() {
        let (store, events, _dir) = fixture();
        let id = SessionId::ephemeral("doctor-holes");
        store.get_or_create(&id).await.unwrap();
        for seq in 1..=3u64 {
            events
                .append(&id, seq, &user(&format!("m{seq}")), seq as i64)
                .await
                .unwrap();
        }

        let projector = crate::gateway::session_projector::MessageProjector::with_event_store(
            store.clone(),
            None,
            Some(events.clone()),
        );
        let check = ProjectionHolesCheck::new(Some(projector), Some(events.clone()));

        let inspect = check.run(Posture::Inspect).await;
        assert_eq!(inspect[0].severity, Severity::Warning);
        assert!(inspect[0].repairable);
        assert!(
            inspect[0].detail.contains("3 event(s)"),
            "the count must be exact, got {:?}",
            inspect[0].detail
        );
        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "inspect must not write"
        );

        let fixed = check.run(Posture::Fix).await;
        assert!(
            matches!(
                fixed[0].repair_outcome,
                Some(RepairOutcome::Repaired { .. })
            ),
            "got {:?}",
            fixed[0].repair_outcome
        );
        assert_eq!(
            store.get_history(&id, None).await.unwrap().len(),
            3,
            "the repair must put the rows in"
        );

        // And the next inspect is clean — the repair is verified by the same
        // predicate that found the problem.
        let after = check.run(Posture::Inspect).await;
        assert!(!after[0].is_problem(), "got {:?}", after[0]);
    }

    /// A session whose transcript matches its log must not be reported. This is
    /// the arm that decides whether the check is a gate or a constant.
    #[tokio::test]
    async fn a_whole_session_is_not_reported() {
        let (store, events, _dir) = fixture();
        let id = SessionId::ephemeral("doctor-whole");
        store.get_or_create(&id).await.unwrap();
        for seq in 1..=2u64 {
            events
                .append(&id, seq, &user(&format!("m{seq}")), seq as i64)
                .await
                .unwrap();
        }
        let projector = crate::gateway::session_projector::MessageProjector::with_event_store(
            store.clone(),
            None,
            Some(events.clone()),
        );
        let check = ProjectionHolesCheck::new(Some(projector.clone()), Some(events.clone()));
        assert!(!projector.request_repair(&id).await.errored);

        let findings = check.run(Posture::Inspect).await;
        assert!(!findings[0].is_problem(), "got {:?}", findings[0]);
    }
}
