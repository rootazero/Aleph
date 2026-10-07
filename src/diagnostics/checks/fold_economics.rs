//! `core/fold-economics` — the lazy fold ledger, surfaced as a doctor check.
//!
//! This check answers a different question from `core/cache-hit-rate`
//! ([`super::cache_hit_rate`]): hit-rate asks "what share of prompt tokens
//! came from the prefix cache"; this ledger asks "**this** fold — did it
//! net-save tokens, or did the summary cost more than the retired span will
//! ever repay?" The two are deliberately not merged: a 97% hit rate coexists
//! happily with a fold that loses tokens on every turn.
//!
//! Everything is computed lazily at query time — no background sweep, no
//! hot-path cost. The inputs already exist on disk: `fold_recorded` /
//! `compaction_performed` rows in `sessions.db` (read back through the fold
//! registry's single source, [`list_folds`]) and `provider_usage` rows in
//! `state.db`'s `task_traces` (the `compactor:<agent>` metering channel, FL
//! §2.18). Both files are opened read-only inside `spawn_blocking`, the same
//! discipline as `core/cache-hit-rate`.
//!
//! Attribution caveat: the metering channel is agent-labelled, not
//! fold-labelled — one compaction emits one `compactor:<agent>` usage row
//! with no fold id on it. A row is therefore attributed to a fold by time
//! proximity ([`ATTRIBUTION_BEFORE_SECS`] before / [`ATTRIBUTION_AFTER_SECS`]
//! after the fold's timestamp; the summarizer call completes just before the
//! `FoldRecorded` row is written). Two folds inside one attribution window
//! share the charge. That is an acknowledged approximation, documented here
//! so nobody "fixes" it by inventing a join key that does not exist.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rusqlite::{params, Connection, OpenFlags};

use crate::context::compact::fold_ledger::{judge_fold, FoldVerdict, Payback};
use crate::context::compact::folds::{list_folds, FoldRecord};
use crate::diagnostics::check::{HealthCheck, Posture, Presence};
use crate::diagnostics::finding::{Finding, Severity};
use crate::session::events::SessionEvent;

const ID: &str = "core/fold-economics";
const TITLE: &str = "Fold economics";
const SESSIONS_DB_FILENAME: &str = "sessions.db";
const STATE_DB_FILENAME: &str = "state.db";
/// Same 24h window as `core/cache-hit-rate`, so the two checks read the same
/// recent past while answering their different questions.
const WINDOW_SECS: i64 = 24 * 60 * 60;
/// Observation horizon N (spec §line-1 1c): a fold is judged over at most the
/// first N turns after it.
const WINDOW_TURNS: u32 = 10;
/// Documented health line: the compactor's own bill should stay under this
/// share of prompt tokens (spec §line-1 1c).
const COMPACTION_COST_HEALTH_PCT: f64 = 2.0;
/// The prefix-cache health line, owned and judged by `core/cache-hit-rate`;
/// repeated in this check's detail as context, never re-judged here.
const HIT_RATE_HEALTH_TEXT: &str = "95–97%";
/// How far before a fold's timestamp a `compactor:<agent>` usage row may sit
/// and still be attributed to that fold.
const ATTRIBUTION_BEFORE_SECS: i64 = 600;
/// How far after a fold's timestamp an unattributed compactor row may sit.
const ATTRIBUTION_AFTER_SECS: i64 = 120;
/// Cap on individually named folds in a finding's detail.
const NAMED_LIMIT: usize = 10;

/// Doctor check judging each recent fold's token payback.
pub struct FoldEconomicsCheck {
    sessions_db: PathBuf,
    state_db: PathBuf,
}

impl FoldEconomicsCheck {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            sessions_db: data_dir.join(SESSIONS_DB_FILENAME),
            state_db: data_dir.join(STATE_DB_FILENAME),
        }
    }

    fn rollup(sessions_db: &Path, state_db: &Path) -> Result<LedgerRollup, String> {
        let now_s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("system clock before unix epoch: {e}"))?
            .as_secs() as i64;
        let cutoff_s = now_s - WINDOW_SECS;
        let folds = load_folds(sessions_db, cutoff_s * 1000)?;
        let usage = load_usage(state_db, cutoff_s)?;
        let mut verdicts = Vec::new();
        let mut unaccounted_folds = 0usize;
        for fold in &folds {
            // Pre-registry folds carry zeroed accounting by construction
            // (see `list_folds`); judging them would read "no accounting" as
            // "saves nothing per turn" — a `Never` false positive. Skip and
            // count them instead.
            if fold.folded_tokens == 0 && fold.summary_tokens == 0 {
                unaccounted_folds += 1;
                continue;
            }
            let fold_at_s = fold.at / 1000;
            let cost_tokens: u64 = usage
                .iter()
                .filter(|u| u.is_compactor)
                .filter(|u| {
                    (fold_at_s - ATTRIBUTION_BEFORE_SECS..=fold_at_s + ATTRIBUTION_AFTER_SECS)
                        .contains(&u.at_s)
                })
                .map(|u| u.billed_tokens)
                .sum();
            let turns_observed = usage
                .iter()
                .filter(|u| !u.is_compactor && u.at_s > fold_at_s)
                .count() as u32;
            verdicts.push(judge_fold(fold, turns_observed, cost_tokens, WINDOW_TURNS));
        }
        let compactor_tokens = usage
            .iter()
            .filter(|u| u.is_compactor)
            .map(|u| u.billed_tokens)
            .sum();
        let prompt_tokens = usage
            .iter()
            .filter(|u| !u.is_compactor)
            .map(|u| u.prompt_tokens)
            .sum();
        Ok(LedgerRollup {
            verdicts,
            unaccounted_folds,
            compactor_tokens,
            prompt_tokens,
        })
    }

    fn report(rollup: &LedgerRollup) -> Finding {
        if rollup.verdicts.is_empty() {
            let detail = if rollup.unaccounted_folds == 0 {
                "No folds recorded in the last 24h — nothing to judge.".to_string()
            } else {
                format!(
                    "{} fold(s) in the last 24h predate the fold registry and carry no token \
                     accounting — nothing to judge.",
                    rollup.unaccounted_folds
                )
            };
            return Finding::ok(ID, "No folds to judge in the last 24h", detail);
        }
        let breakeven = rollup
            .verdicts
            .iter()
            .filter(|v| v.payback == Payback::Breakeven)
            .count();
        let not_yet = rollup
            .verdicts
            .iter()
            .filter(|v| v.payback == Payback::NotYet)
            .count();
        let never = rollup
            .verdicts
            .iter()
            .filter(|v| v.payback == Payback::Never)
            .count();
        let mut lines = Vec::new();
        for verdict in rollup.verdicts.iter().take(NAMED_LIMIT) {
            let word = match verdict.payback {
                Payback::Breakeven => "breakeven",
                Payback::NotYet => "not-yet",
                Payback::Never => "never",
            };
            lines.push(format!(
                "{}: net {:+} tok ({}; summarizer charged {} tok)",
                verdict.fold_id, verdict.net_saved_tokens, word, verdict.summary_cost_tokens
            ));
        }
        if rollup.verdicts.len() > NAMED_LIMIT {
            lines.push(format!(
                "… and {} more",
                rollup.verdicts.len() - NAMED_LIMIT
            ));
        }
        if rollup.prompt_tokens > 0 {
            let share = 100.0 * rollup.compactor_tokens as f64 / rollup.prompt_tokens as f64;
            lines.push(format!(
                "compactor self-cost {} tok = {share:.1}% of {} prompt tok (health line \
                 ≤{COMPACTION_COST_HEALTH_PCT:.0}%)",
                rollup.compactor_tokens, rollup.prompt_tokens
            ));
        } else {
            lines.push(format!(
                "compactor self-cost {} tok (no prompt traffic in window — share n/a; health \
                 line ≤{COMPACTION_COST_HEALTH_PCT:.0}%)",
                rollup.compactor_tokens
            ));
        }
        lines.push(format!(
            "health lines: prefix-cache hit rate {HIT_RATE_HEALTH_TEXT} is tracked by \
             core/cache-hit-rate (not re-judged here); this ledger's own line is compaction \
             self-cost ≤{COMPACTION_COST_HEALTH_PCT:.0}% of prompt tokens"
        ));
        if rollup.unaccounted_folds > 0 {
            lines.push(format!(
                "{} pre-registry fold(s) carry no token accounting and were skipped",
                rollup.unaccounted_folds
            ));
        }
        let detail = lines.join("\n");
        let total = rollup.verdicts.len();
        if never > 0 {
            Finding::problem(
                ID,
                Severity::Warning,
                format!("{never} fold(s) in the last 24h cost more than they save"),
                detail,
            )
            .with_fix_hint(
                "a fold whose summary costs ≥ the span it retired can never pay back — inspect \
                 what inflated the summary (degraded summarizer? pathological span); \
                 core/cache-hit-rate covers the aggregate cache picture",
            )
        } else {
            Finding::ok(
                ID,
                format!("{total} fold(s) judged in the last 24h: {breakeven} breakeven, {not_yet} not-yet"),
                detail,
            )
        }
    }
}

/// One `provider_usage` row, pre-digested.
struct UsageRow {
    at_s: i64,
    /// `agent_id` starts with `compactor:` — the cheap-summarizer metering
    /// channel (FL §2.18).
    is_compactor: bool,
    /// What the call was billed: input + output tokens.
    billed_tokens: u64,
    /// Prompt-side tokens that flowed through the provider:
    /// input + cache read + cache creation.
    prompt_tokens: u64,
}

struct LedgerRollup {
    verdicts: Vec<FoldVerdict>,
    unaccounted_folds: usize,
    compactor_tokens: u64,
    prompt_tokens: u64,
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|e| format!("{e}"))
}

/// Read the last window's folds out of `sessions.db`, grouped per session and
/// reconstructed through the registry's single source (`list_folds`) so the
/// doctor check can never drift from what the runtime believes a fold is.
fn load_folds(sessions_db: &Path, cutoff_ms: i64) -> Result<Vec<FoldRecord>, String> {
    let conn = Connection::open_with_flags(sessions_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", sessions_db.display()))?;
    if !table_exists(&conn, "session_events")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT session_id, payload_json FROM session_events \
             WHERE event_type IN ('fold_recorded', 'compaction_performed') \
             AND created_at >= ?1 ORDER BY session_id, seq",
        )
        .map_err(|e| format!("{e}"))?;
    let rows = stmt
        .query_map(params![cutoff_ms], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("{e}"))?;
    let mut by_session: BTreeMap<String, Vec<SessionEvent>> = BTreeMap::new();
    for row in rows {
        let (session_id, payload) = row.map_err(|e| format!("{e}"))?;
        // Undecodable payloads (a newer build's event shape) are skipped, not
        // fatal: one bad row must not blind the whole ledger.
        if let Ok(event) = serde_json::from_str::<SessionEvent>(&payload) {
            by_session.entry(session_id).or_default().push(event);
        }
    }
    let mut folds = Vec::new();
    for (session_key, events) in &by_session {
        folds.extend(list_folds(session_key, events));
    }
    Ok(folds)
}

/// Read the last window's `provider_usage` rows out of `state.db`. A missing
/// `task_traces` table means "no telemetry yet", not an error.
fn load_usage(state_db: &Path, cutoff_s: i64) -> Result<Vec<UsageRow>, String> {
    let conn = Connection::open_with_flags(state_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", state_db.display()))?;
    if !table_exists(&conn, "task_traces")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT timestamp, \
             COALESCE(json_extract(event_json, '$.agent_id'), ''), \
             CAST(COALESCE(json_extract(event_json, '$.input_tokens'), 0) AS INTEGER), \
             CAST(COALESCE(json_extract(event_json, '$.output_tokens'), 0) AS INTEGER), \
             CAST(COALESCE(json_extract(event_json, '$.cache_read_tokens'), 0) AS INTEGER), \
             CAST(COALESCE(json_extract(event_json, '$.cache_creation_tokens'), 0) AS INTEGER) \
             FROM task_traces WHERE event_kind = 'provider_usage' AND timestamp >= ?1",
        )
        .map_err(|e| format!("{e}"))?;
    let rows = stmt
        .query_map(params![cutoff_s], |row| {
            let at_s = row.get::<_, i64>(0)?;
            let agent = row.get::<_, String>(1)?;
            let input = row.get::<_, i64>(2)? as u64;
            let output = row.get::<_, i64>(3)? as u64;
            let read = row.get::<_, i64>(4)? as u64;
            let creation = row.get::<_, i64>(5)? as u64;
            Ok(UsageRow {
                at_s,
                is_compactor: agent.starts_with("compactor:"),
                billed_tokens: input + output,
                prompt_tokens: input + read + creation,
            })
        })
        .map_err(|e| format!("{e}"))?;
    let mut usage = Vec::new();
    for row in rows {
        usage.push(row.map_err(|e| format!("{e}"))?);
    }
    Ok(usage)
}

#[async_trait]
impl HealthCheck for FoldEconomicsCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        TITLE
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        match Presence::of(ID, TITLE, &self.sessions_db) {
            Err(finding) => return vec![finding],
            Ok(Presence::Absent) => {
                return vec![Finding::ok(
                    ID,
                    "No session database yet",
                    "sessions.db is absent — no session has been recorded on this data dir, so \
                     there are no folds to judge.",
                )];
            }
            Ok(Presence::Present) => {}
        }
        match Presence::of(ID, TITLE, &self.state_db) {
            Err(finding) => return vec![finding],
            Ok(Presence::Absent) => {
                return vec![Finding::ok(
                    ID,
                    "No trace database yet",
                    "state.db is absent — the metering channel has recorded no provider usage, \
                     so fold payback cannot be judged yet.",
                )];
            }
            Ok(Presence::Present) => {}
        }
        let sessions_db = self.sessions_db.clone();
        let state_db = self.state_db.clone();
        let outcome =
            tokio::task::spawn_blocking(move || Self::rollup(&sessions_db, &state_db)).await;
        let rollup = match outcome {
            Ok(Ok(rollup)) => rollup,
            Ok(Err(err)) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Fold ledger unreadable",
                    err,
                )
                .with_fix_hint(
                    "sessions.db / state.db exist but could not be queried — check file \
                     permissions and that no second aleph-server process holds them",
                )];
            }
            Err(join) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Fold ledger unreadable",
                    format!("ledger query task failed: {join}"),
                )];
            }
        };
        vec![Self::report(&rollup)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_s() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    fn fold_event(
        fold_id: &str,
        from_seq: u64,
        to_seq: u64,
        folded_tokens: u64,
        summary_tokens: u64,
        at_ms: i64,
    ) -> SessionEvent {
        SessionEvent::FoldRecorded {
            fold_id: fold_id.to_string(),
            from_seq,
            to_seq,
            summary_ref: "turn-of-summary".to_string(),
            strategy: "manual".to_string(),
            trigger: "manual-command".to_string(),
            folded_tokens,
            summary_tokens,
            at: at_ms,
        }
    }

    fn create_sessions_db(path: &Path, rows: &[(i64, SessionEvent)]) {
        let conn = Connection::open(path).expect("create sessions db");
        conn.execute_batch(
            "CREATE TABLE session_events (\
             session_id TEXT NOT NULL, seq INTEGER NOT NULL, turn_id TEXT, \
             event_type TEXT NOT NULL, payload_json TEXT NOT NULL, \
             created_at INTEGER NOT NULL, retired_at INTEGER, \
             PRIMARY KEY (session_id, seq));",
        )
        .expect("create session_events");
        for (seq, event) in rows {
            let payload = serde_json::to_string(event).expect("encode event");
            let event_type = match event {
                SessionEvent::FoldRecorded { .. } => "fold_recorded",
                SessionEvent::CompactionPerformed { .. } => "compaction_performed",
                other => panic!("fixture only encodes fold events, got {other:?}"),
            };
            conn.execute(
                "INSERT INTO session_events \
                 (session_id, seq, turn_id, event_type, payload_json, created_at) \
                 VALUES ('s', ?1, NULL, ?2, ?3, ?4)",
                params![seq, event_type, payload, now_s() * 1000],
            )
            .expect("insert fold row");
        }
    }

    struct UsageFixture {
        agent: &'static str,
        at_s: i64,
        input: u64,
        output: u64,
        read: u64,
        creation: u64,
    }

    fn create_state_db(path: &Path, rows: &[UsageFixture]) {
        let conn = Connection::open(path).expect("create state db");
        conn.execute_batch(
            "CREATE TABLE task_traces (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, \
             step_index INTEGER NOT NULL, event_kind TEXT NOT NULL, \
             event_json TEXT NOT NULL, timestamp INTEGER NOT NULL);",
        )
        .expect("create task_traces");
        for (i, u) in rows.iter().enumerate() {
            let json = format!(
                "{{\"kind\":\"provider_usage\",\"agent_id\":\"{}\",\"input_tokens\":{},\
                 \"output_tokens\":{},\"cache_read_tokens\":{},\"cache_creation_tokens\":{},\
                 \"thinking_tokens\":0}}",
                u.agent, u.input, u.output, u.read, u.creation
            );
            conn.execute(
                "INSERT INTO task_traces \
                 (task_id, step_index, event_kind, event_json, timestamp) \
                 VALUES ('t', ?1, 'provider_usage', ?2, ?3)",
                params![i as i64, json, u.at_s],
            )
            .expect("insert usage row");
        }
    }

    fn check_with_dbs(
        dir: &tempfile::TempDir,
        folds: &[(i64, SessionEvent)],
        usage: &[UsageFixture],
    ) -> FoldEconomicsCheck {
        create_sessions_db(&dir.path().join(SESSIONS_DB_FILENAME), folds);
        create_state_db(&dir.path().join(STATE_DB_FILENAME), usage);
        FoldEconomicsCheck::new(dir.path().to_path_buf())
    }

    /// One fold whose reads dominate (breakeven) and one inverted fold
    /// (summary ≥ retired span — never) must both reach the finding, labelled
    /// with their verdicts.
    #[tokio::test]
    async fn breakeven_and_never_folds_reach_the_finding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = now_s();
        let folds = vec![
            (1, fold_event("fold_a_2", 2, 40, 12_000, 800, now * 1000)),
            (2, fold_event("fold_b_41", 41, 80, 800, 900, now * 1000)),
        ];
        let usage = vec![
            // The summarizer bill: one compactor row just before both folds.
            UsageFixture {
                agent: "compactor:a",
                at_s: now - 60,
                input: 1_000,
                output: 200,
                read: 0,
                creation: 0,
            },
            // Two main-agent turns after the folds.
            UsageFixture {
                agent: "a",
                at_s: now + 5,
                input: 5_000,
                output: 500,
                read: 40_000,
                creation: 2_000,
            },
            UsageFixture {
                agent: "a",
                at_s: now + 10,
                input: 5_000,
                output: 500,
                read: 40_000,
                creation: 2_000,
            },
        ];
        let check = check_with_dbs(&dir, &folds, &usage);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1, "one finding expected: {findings:?}");
        let finding = &findings[0];
        // fold_b is inverted (summary 900 ≥ folded 800) → Never → the check
        // must warn.
        assert_eq!(finding.severity, Severity::Warning);
        assert!(finding.is_problem());
        // Effect-reached assertions: both folds named, with their verdicts
        // and the breakeven fold's net position.
        assert!(finding.detail.contains("fold_a_2"), "{}", finding.detail);
        assert!(finding.detail.contains("fold_b_41"), "{}", finding.detail);
        assert!(finding.detail.contains("breakeven"), "{}", finding.detail);
        assert!(finding.detail.contains("never"), "{}", finding.detail);
        // fold_a: 2 turns × 11,200 − 1,200 = +21,200.
        assert!(finding.detail.contains("+21200"), "{}", finding.detail);
    }

    #[tokio::test]
    async fn empty_db_yields_informational_not_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let check = check_with_dbs(&dir, &[], &[]);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1, "one finding expected: {findings:?}");
        let finding = &findings[0];
        assert!(!finding.is_problem(), "empty db must not warn: {finding:?}");
        assert_eq!(finding.severity, Severity::Info);
        assert_eq!(finding.title, "No folds to judge in the last 24h");
    }

    #[tokio::test]
    async fn absent_databases_yield_informational_not_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let check = FoldEconomicsCheck::new(dir.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1, "one finding expected: {findings:?}");
        assert!(
            !findings[0].is_problem(),
            "absent dbs must not warn: {findings:?}"
        );
        assert_eq!(findings[0].title, "No session database yet");
    }

    #[tokio::test]
    async fn health_lines_documented() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = now_s();
        let folds = vec![(1, fold_event("fold_a_2", 2, 40, 12_000, 800, now * 1000))];
        let usage = vec![UsageFixture {
            agent: "compactor:a",
            at_s: now - 60,
            input: 1_000,
            output: 200,
            read: 0,
            creation: 0,
        }];
        let check = check_with_dbs(&dir, &folds, &usage);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1, "one finding expected: {findings:?}");
        let detail = &findings[0].detail;
        // The two documented health lines from the spec: prefix-cache hit
        // rate 95–97% and compaction self-cost ≤2% of prompt tokens.
        assert!(detail.contains("95–97%"), "{detail}");
        assert!(detail.contains("≤2%"), "{detail}");
    }
}
