//! The `cron.list` row contract, shared by the gateway handler that produces
//! it and every client that renders it.
//!
//! # Why this type exists
//!
//! `cron.list` used to be a hand-written `json!` map in
//! `gateway/handlers/cron/real.rs`, and each client guessed at its shape. The
//! CLI's private `struct CronJob` declared a REQUIRED `schedule: String` — a
//! key the server has never emitted — so `serde_json::from_value::<Vec<_>>`
//! returned `Err` for any non-empty job list, `.unwrap_or_default()` folded
//! that into an empty `Vec`, and `aleph cron list` printed
//! "No cron jobs configured" with exit code 0 on a server with jobs. Its
//! `description` / `last_run` / `next_run` columns were dead for the same
//! reason (the server sends `name` / `last_run_at` / `next_run_at`), so even a
//! decode that had succeeded would have rendered three columns of dashes.
//!
//! This is the fifth instance of that class in this repo, and the fix is the
//! same DIRECTION the others took: the server **constructs** [`CronJobRow`] and
//! serializes it, rather than hand-writing a map that a client parses *against*
//! the type. Parsing can only ever prove "the response is a superset of what I
//! need"; construction makes over-sending inexpressible, and the key-set
//! equality guard in this module's tests makes a server-side rename fail at
//! `cargo test` instead of at an operator's terminal.
//!
//! # Why some fields are `Value`
//!
//! `schedule_kind`, `last_error_reason`, `failure_alert` and `chain` are owned
//! by `alephcore` (`tasks::cron::{ScheduleKind, ErrorReason, …}`) and this
//! crate deliberately has no runtime dependencies, so it cannot name them.
//! They travel verbatim; [`CronJobRow::schedule_summary`] is the one reader of
//! `schedule_kind`'s tagged shape so no client re-derives it.
//!
//! # Why nothing here carries `#[serde(default)]`
//!
//! Deliberate, and the same reason [`crate::trace_replay::AgentTraceListRow`]
//! gives: a default turns a server-side rename into a silent column of dashes,
//! which reads as "no value yet" rather than as a broken contract. A missing
//! key must be loud.
//!
//! ⚠️ That only reaches the non-nullable half. serde gives every `Option<T>`
//! field an implicit default — a missing key deserializes to `None` with no
//! attribute written — so absence is unmakeable-loud for `last_run_at` and its
//! siblings no matter what is written here. The guard below derives which keys
//! those are instead of listing them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One row of `cron.list`, and the payload of `cron.get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Derived, read-only: the job is switched on but has nothing scheduled,
    /// so it will never fire again until an operator acts.
    ///
    /// `enabled` alone cannot express this — a permanently-failed job is parked
    /// by clearing its next run and leaving the switch on, so a surface that
    /// shows only `enabled` shows a healthy-looking job that is in fact dead.
    /// `CronJobView::parked` has computed this since the field was introduced;
    /// it had simply never been placed on the wire, so no RPC client could ask
    /// the question the field exists to answer.
    pub parked: bool,
    /// The tagged `ScheduleKind` object: `{"kind":"cron","expr":…,"tz":…}`,
    /// `{"kind":"every","every_ms":…}` or `{"kind":"at","at":…}`.
    /// Read it through [`CronJobRow::schedule_summary`].
    pub schedule_kind: Value,
    pub agent_id: String,
    pub source_channel_id: Option<String>,
    pub prompt: String,
    /// Projected from `ScheduleKind::Cron { tz }`; `None` for interval and
    /// one-shot schedules, which are absolute.
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    /// `"main"` or `"isolated"`.
    pub session_target: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Epoch **milliseconds**. `None` once the job is parked or disabled.
    pub next_run_at: Option<i64>,
    /// Epoch milliseconds; `Some` only while a run is in flight.
    pub running_at_ms: Option<i64>,
    /// Epoch milliseconds of the last completed run.
    pub last_run_at: Option<i64>,
    /// `"ok"` / `"error"` / `"skipped"` / `"timeout"`.
    pub last_run_status: Option<String>,
    pub last_error: Option<String>,
    /// The tagged `ErrorReason`: `{"kind":"transient"|"permanent","message":…}`.
    pub last_error_reason: Option<Value>,
    pub last_duration_ms: Option<i64>,
    pub consecutive_errors: u32,
    /// `"delivered"` / `"not_delivered"` / `"not_requested"`.
    pub last_delivery_status: Option<String>,
    pub failure_alert: Option<Value>,
    pub chain: Option<Value>,
    pub timeout_ms: Option<i64>,
}

impl CronJobRow {
    /// A one-line, human-facing rendering of [`Self::schedule_kind`].
    ///
    /// Lives here rather than in a client because the tagged shape has exactly
    /// one owner: a second reader is a second answer to "what does this job's
    /// schedule say", and the CLI's previous answer — a `schedule` key that did
    /// not exist — is what this module was written to end.
    #[must_use]
    pub fn schedule_summary(&self) -> String {
        let kind = self
            .schedule_kind
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "cron" => {
                let expr = self
                    .schedule_kind
                    .get("expr")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                match self.timezone.as_deref() {
                    Some(tz) => format!("cron {expr} ({tz})"),
                    None => format!("cron {expr}"),
                }
            }
            "every" => {
                let ms = self
                    .schedule_kind
                    .get("every_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                format!("every {ms}ms")
            }
            "at" => {
                let at = self
                    .schedule_kind
                    .get("at")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                format!("at {at}")
            }
            // Not a fallback for a *missing* schedule: `schedule_kind` is
            // required, so reaching here means the server grew a variant this
            // client does not know. Show the raw tag rather than inventing a
            // schedule.
            other if !other.is_empty() => format!("{other} (unknown kind)"),
            _ => "-".to_string(),
        }
    }
}

/// The `cron.list` response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronListResponse {
    pub jobs: Vec<CronJobRow>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> CronJobRow {
        CronJobRow {
            id: "job-1".to_string(),
            name: "daily brief".to_string(),
            enabled: true,
            parked: false,
            schedule_kind: json!({"kind": "cron", "expr": "0 0 8 * * *", "tz": "UTC"}),
            agent_id: "main".to_string(),
            source_channel_id: None,
            prompt: "summarise".to_string(),
            timezone: Some("UTC".to_string()),
            tags: vec!["brief".to_string()],
            session_target: "isolated".to_string(),
            created_at: 1,
            updated_at: 2,
            next_run_at: Some(3),
            running_at_ms: None,
            last_run_at: Some(4),
            last_run_status: Some("ok".to_string()),
            last_error: None,
            last_error_reason: None,
            last_duration_ms: Some(50),
            consecutive_errors: 0,
            last_delivery_status: Some("delivered".to_string()),
            failure_alert: None,
            chain: None,
            timeout_ms: None,
        }
    }

    /// A row with every optional absent — the derivation source for which
    /// keys serde can be made to demand.
    fn all_optionals_none() -> CronJobRow {
        CronJobRow {
            source_channel_id: None,
            timezone: None,
            next_run_at: None,
            running_at_ms: None,
            last_run_at: None,
            last_run_status: None,
            last_error: None,
            last_error_reason: None,
            last_duration_ms: None,
            last_delivery_status: None,
            failure_alert: None,
            chain: None,
            timeout_ms: None,
            ..sample()
        }
    }

    /// Every key must survive a round trip, and no NON-NULLABLE key may
    /// acquire a `#[serde(default)]`.
    ///
    /// The CLI's dead `schedule` / `description` / `last_run` / `next_run`
    /// columns are what a default looks like in production: a column of dashes
    /// that reads as "no value yet". Where serde permits it, a server-side
    /// rename must instead be a loud parse error at the client.
    ///
    /// The nullable exemption is DERIVED, not listed: serialize a row whose
    /// optionals are all `None` and the keys that come out `null` are exactly
    /// the ones serde will default. A hand-written exemption list would rot
    /// into a standing permission for a field that stopped being optional.
    #[test]
    fn every_required_cron_row_key_is_demanded() {
        let value = serde_json::to_value(sample()).expect("serialize");
        let object = value.as_object().expect("row is an object").clone();
        assert_eq!(
            object.len(),
            25,
            "CronJobRow shape changed — update this guard and the CLI renderer"
        );

        let all_none = serde_json::to_value(all_optionals_none()).expect("serialize");
        let nullable: Vec<String> = all_none
            .as_object()
            .expect("row is an object")
            .iter()
            .filter(|(_, v)| v.is_null())
            .map(|(k, _)| k.clone())
            .collect();

        let mut checked = 0usize;
        for key in object.keys() {
            if nullable.contains(key) {
                continue;
            }
            let mut broken = object.clone();
            broken.remove(key);
            assert!(
                serde_json::from_value::<CronJobRow>(Value::Object(broken)).is_err(),
                "dropping `{key}` still parsed — it has picked up a serde default, \
                 so a server that stops sending it renders as an empty value \
                 instead of failing loudly"
            );
            checked += 1;
        }
        // Self-check: the derived exemption must not have swallowed the row.
        assert_eq!(checked, object.len() - nullable.len());
        assert!(
            checked >= 10,
            "only {checked} required keys were checked — the derivation is wrong"
        );
    }

    #[test]
    fn schedule_summary_reads_every_tagged_variant() {
        let mut row = sample();
        assert_eq!(row.schedule_summary(), "cron 0 0 8 * * * (UTC)");

        row.timezone = None;
        row.schedule_kind = json!({"kind": "cron", "expr": "*/5 * * * * *"});
        assert_eq!(row.schedule_summary(), "cron */5 * * * * *");

        row.schedule_kind = json!({"kind": "every", "every_ms": 60_000});
        assert_eq!(row.schedule_summary(), "every 60000ms");

        row.schedule_kind = json!({"kind": "at", "at": 1_700_000_000});
        assert_eq!(row.schedule_summary(), "at 1700000000");

        // A variant this client has not learned yet must say so rather than
        // render as "no schedule".
        row.schedule_kind = json!({"kind": "solar"});
        assert_eq!(row.schedule_summary(), "solar (unknown kind)");
    }
}
