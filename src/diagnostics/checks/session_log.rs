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
//! **Report-only, deliberately.** There is no `repairable()` here and there
//! must not be one: a log whose markers contradict each other cannot be
//! mechanically resolved without deciding which of two disagreeing records is
//! the truth, and that decision belongs to whoever reads the transcript. The
//! neighbouring `core/projection-holes` IS repairable because a missing
//! projection row has exactly one correct value — the event it was derived
//! from. This one does not.
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
use crate::diagnostics::finding::{Finding, Severity};
use crate::gateway::session_projector::MessageProjector;
use crate::gateway::session_store::types::SessionFilter;
use crate::session::reduction::{reduce_run, LogContradiction};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

const ID: &str = "core/session-log";
/// Noun phrase the "unknown" finding is titled with — `"Session log unknown"`.
const SUBJECT: &str = "Session log";

/// How many contradicting sessions to name in the detail. The counts stay
/// exact; only the roll-call is capped.
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
    /// `true` when the reducer REFUSED the log (`Err`) rather than reading it
    /// with a stated correction. The two are different answers to the operator
    /// — a refused log is why a resume did nothing.
    refused: bool,
    tags: Vec<&'static str>,
}

#[async_trait]
impl HealthCheck for SessionLogCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Session log"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
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
                    format!("the run markers could not be read ({e}), so no session's log was reduced."),
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
        for key in keys.keys() {
            let Some(id) = SessionId::from_key_string(key) else {
                unreadable += 1;
                continue;
            };
            let Ok(log) = events.load_all_events(&id).await else {
                unreadable += 1;
                continue;
            };
            match reduce_run(&log) {
                Err(c) => bad.push(Contradicting {
                    key: key.clone(),
                    refused: true,
                    tags: vec![c.tag()],
                }),
                Ok(r) if !r.contradictions.is_empty() => {
                    let mut tags: Vec<&'static str> =
                        r.contradictions.iter().map(LogContradiction::tag).collect();
                    tags.dedup();
                    bad.push(Contradicting {
                        key: key.clone(),
                        refused: false,
                        tags,
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
                format!("{scanned} session log(s) reduced; none contradicts itself."),
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
                        .map_or_else(String::new, |e| format!(", and the marker scan failed ({e})"))
                ),
            )];
        }

        let refused = bad.iter().filter(|b| b.refused).count();
        let named: Vec<String> = bad
            .iter()
            .take(NAMED_LIMIT)
            .map(|b| {
                format!(
                    "{} [{}{}]",
                    b.key,
                    if b.refused { "refused: " } else { "" },
                    b.tags.join(", ")
                )
            })
            .chain(
                (bad.len() > NAMED_LIMIT)
                    .then(|| format!("… and {} more", bad.len() - NAMED_LIMIT)),
            )
            .collect();

        vec![Finding::problem(
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
                "{} of {scanned} session log(s) contradict themselves{}: {}.{}",
                bad.len(),
                if refused > 0 {
                    format!(", {refused} of them refused by the reducer")
                } else {
                    String::new()
                },
                named.join("; "),
                if unreadable > 0 {
                    format!(" A further {unreadable} could not be reduced.")
                } else {
                    String::new()
                }
            ),
        )
        .with_fix_hint(
            "Not mechanically repairable: resolving a contradiction means deciding which \
             of two disagreeing records is true, which this check cannot do for you. A \
             `refused` log is why `aleph resume` answered `log_inconsistent` for that \
             session — read the named seq in the transcript. A reported kind was worked \
             around with a stated reading and costs nothing but the note.",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// grep for.
    #[test]
    fn every_contradiction_tag_belongs_to_this_checks_namespace() {
        use LogContradiction as C;
        let all = [
            C::OutOfOrderSlice { at_seq: 1 },
            C::NonMarkerInMarkerSlice { seq: 1 },
        ];
        for c in all {
            assert!(
                c.tag().starts_with("session-log-"),
                "tag {} is outside this check's namespace",
                c.tag()
            );
        }
    }
}
