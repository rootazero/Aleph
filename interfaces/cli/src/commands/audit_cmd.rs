//! `aleph audit` — read the security audit trail.
//!
//! ## Why this command exists
//!
//! `security_audit_log` had five writers and no reader. The `AuthorityChange`
//! entries in particular exist so that "who changed who could do what, and
//! when" is answerable after an incident; until this command the answer was
//! `sqlite3` against a file inside `~/.aleph/data`, which is both a
//! sandbox-wall crossing this repo has already removed once and a read with no
//! retention horizon attached to it.
//!
//! ## Why the CLI
//!
//! `security.*` is admin-gated, and the CLI reaches the server over loopback,
//! which resolves to the implicit owner as `"operator"` — the same argument
//! that put `aleph users` here rather than in the Panel, and the same
//! consequence: reading the trail means being at the machine. For the verb that
//! reads every principal's authority history, that is the posture worth having.

use aleph_protocol::audit::{AuditQueryParams, AuditQueryResult};
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// Parse a `--since` window: a bare number is seconds, or a `s`/`m`/`h`/`d`
/// suffix.
///
/// Rejected rather than defaulted on a bad unit: `--since 7w` silently read as
/// 7 seconds would answer a much narrower question than the operator asked,
/// and a narrower audit answer is indistinguishable from a quiet one.
pub fn parse_since(spec: &str) -> Result<i64, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty --since".to_string());
    }
    let (digits, mult) = match spec.chars().last() {
        Some('s') => (&spec[..spec.len() - 1], 1),
        Some('m') => (&spec[..spec.len() - 1], 60),
        Some('h') => (&spec[..spec.len() - 1], 3600),
        Some('d') => (&spec[..spec.len() - 1], 86_400),
        Some(c) if c.is_ascii_digit() => (spec, 1),
        _ => return Err(format!("unrecognised --since unit in {spec:?}; use s, m, h or d")),
    };
    let n: i64 = digits
        .parse()
        .map_err(|_| format!("--since {spec:?} is not a number followed by s, m, h or d"))?;
    if n < 0 {
        return Err("--since must not be negative".to_string());
    }
    n.checked_mul(mult)
        .ok_or_else(|| format!("--since {spec:?} overflows"))
}

/// Query and render the trail.
pub async fn query(
    server_url: &str,
    config: &CliConfig,
    event_type: Option<&str>,
    actor_user: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> CliResult<()> {
    let since_secs = match since {
        Some(s) => Some(parse_since(s).map_err(CliError::Other)?),
        None => None,
    };
    // Built from the contract type, never a hand-written literal: this family's
    // two halves live in two crates, and the `workspace create` contract split
    // is what happens when each side spells the keys itself.
    let params = AuditQueryParams {
        event_type: event_type.map(str::to_string),
        actor_user: actor_user.map(str::to_string),
        since_secs,
        limit,
    };

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("security.audit.query", Some(params)).await?;
    client.close().await?;

    let result: AuditQueryResult =
        serde_json::from_value(raw.clone()).map_err(|e| CliError::Other(e.to_string()))?;

    let rows: Vec<Vec<String>> = result
        .entries
        .iter()
        .map(|e| {
            vec![
                format_timestamp(e.timestamp),
                e.event_type.clone(),
                e.severity.clone(),
                // A dash is "the trail does not know", not "nobody" — the
                // producers that predate the user model write NULL here.
                e.actor_user.clone().unwrap_or_else(|| "-".to_string()),
                e.detail.clone(),
            ]
        })
        .collect();

    output::print_table(
        &["When (UTC)", "Event", "Severity", "Actor", "Detail"],
        &rows,
        json,
        &raw,
    );

    if !json {
        // Both of these are the difference between "the window is clean" and
        // "I am not showing you everything". Printing the rows and swallowing
        // these two facts would be the same defect this command was written to
        // fix, one layer out.
        if result.truncated {
            println!();
            println!(
                "More entries matched than were shown — raise --limit (max {}) or narrow --since.",
                aleph_protocol::audit::MAX_AUDIT_LIMIT
            );
        }
        if result.entries.is_empty() {
            println!();
            println!(
                "No matching entries. The trail keeps {} and is purged behind that, \
                 so an empty window is not proof nothing happened.",
                humanize_secs(result.retention_secs)
            );
        }
    }

    Ok(())
}

/// Render a stored unix-**second** timestamp. The column's unit is seconds
/// (`strftime('%s')`), unlike most of this repo's millisecond timestamps, so
/// the conversion is spelled out here rather than inferred at the call site.
fn format_timestamp(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map_or_else(|| secs.to_string(), |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn humanize_secs(secs: i64) -> String {
    let days = secs / 86_400;
    if days > 0 {
        return format!("{days} day(s)");
    }
    let hours = secs / 3600;
    if hours > 0 {
        return format!("{hours} hour(s)");
    }
    format!("{secs} second(s)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_accepts_the_four_units_and_bare_seconds() {
        assert_eq!(parse_since("90").unwrap(), 90);
        assert_eq!(parse_since("90s").unwrap(), 90);
        assert_eq!(parse_since("5m").unwrap(), 300);
        assert_eq!(parse_since("2h").unwrap(), 7200);
        assert_eq!(parse_since("7d").unwrap(), 604_800);
    }

    /// An unknown unit must be refused, not truncated to its digits: `7w`
    /// read as 7 seconds answers a question the operator did not ask, and the
    /// resulting near-empty page looks exactly like a quiet week.
    #[test]
    fn since_refuses_an_unknown_unit_rather_than_narrowing_the_window() {
        assert!(parse_since("7w").is_err());
        assert!(parse_since("yesterday").is_err());
        assert!(parse_since("").is_err());
        assert!(parse_since("-3h").is_err());
    }

    /// The wire keys the server deserialises are the ones this command sends,
    /// because both come from the same type. Asserting the *contract type's*
    /// own serialisation (rather than a literal written here) is the only
    /// version of this test that can fail when the contract moves.
    #[test]
    fn the_request_carries_only_the_filters_the_operator_gave() {
        let params = AuditQueryParams {
            event_type: Some("authority_change".to_string()),
            actor_user: None,
            since_secs: None,
            limit: None,
        };
        let wire = serde_json::to_value(&params).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"event_type": "authority_change"}),
            "absent filters must not be sent as nulls; a null filter is an \
             assertion about the value, not the absence of one"
        );
    }

    #[test]
    fn retention_is_described_in_the_unit_an_operator_thinks_in() {
        assert_eq!(humanize_secs(30 * 24 * 3600), "30 day(s)");
        assert_eq!(humanize_secs(7200), "2 hour(s)");
        assert_eq!(humanize_secs(45), "45 second(s)");
    }
}
