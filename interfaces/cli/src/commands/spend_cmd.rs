//! `aleph spend` — read the per-principal spend ledger.
//!
//! ## Why this command exists
//!
//! `SpendLedger` records every principal's per-period USD spend the moment
//! `[policies.spend]` ceilings are enforced (the metering floor and the
//! admission gate), but until `spend.query` shipped nothing could read the
//! ledger back out. This command is the headless half of that deliverable:
//! a deployment with no Panel attached can still answer "what has been
//! spent, by whom, in the window that is open now."
//!
//! ## Why the CLI
//!
//! `spend.query` is admin-gated, and the CLI reaches the server over
//! loopback, which resolves to the implicit owner as `"operator"` — the
//! same posture that put `aleph audit` and `aleph users` here rather than
//! in the Panel.

use aleph_protocol::spend::{SpendQueryParams, SpendQueryResult};
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// (display header, wire field name) for every column this command renders,
/// in print order.
///
/// The wire field name half is not decorative: `tests` below asserts it is
/// a real key in a `SpendQueryResult` serialised from the contract type.
/// That is the guard against the defect `aleph providers list` shipped
/// with — a header ("type", "default") that looked plausible but was never
/// backed by anything the server actually sent ("provider_type",
/// "is_default" were the real keys), so every row rendered a dash from the
/// day it was written and nobody noticed because a dash reads as "no value
/// yet," not as a bug.
const COLUMNS: &[(&str, &str)] = &[
    ("Principal", "principal"),
    ("USD", "usd"),
    ("Unpriced Calls", "unpriced_calls"),
    ("Partial Calls", "partial_calls"),
];

/// Query and render the ledger.
pub async fn query(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    // Empty today by design (see `SpendQueryParams`'s doc): there is exactly
    // one window worth asking about, the one that is open right now, for
    // every principal with a row in it.
    let params = SpendQueryParams::default();

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("spend.query", Some(params)).await?;
    client.close().await?;

    let result: SpendQueryResult =
        serde_json::from_value(raw.clone()).map_err(|e| CliError::Other(e.to_string()))?;

    // `configured: false` does not mean zero spend — rows are sent whether
    // or not a ceiling is enforced. Falling straight through to the table
    // would, on a fresh or intentionally-unlimited deployment, print
    // nothing but an empty table, and an empty table reads as "nobody
    // spent anything" — a different fact than "no ceiling is watching,"
    // and the one a reader would actually act on. Say the true fact first,
    // in words, before the table lets rows count be mistaken for it.
    if !json && !result.configured {
        println!(
            "No spend ceiling is configured (`[policies.spend]` is unset) — the figures \
             below are recorded but nothing is being enforced against them."
        );
        println!();
    }

    let headers: Vec<&str> = COLUMNS.iter().map(|(header, _)| *header).collect();
    let rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| {
            vec![
                row.principal.clone(),
                format!("{:.2}", row.usd),
                row.unpriced_calls.to_string(),
                row.partial_calls.to_string(),
            ]
        })
        .collect();

    output::print_table(&headers, &rows, json, &raw);

    if !json {
        println!();
        println!(
            "Period: {} – {}",
            format_ms(result.period_start_ms),
            format_ms(result.period_end_ms)
        );
        // `usd` is a lower bound, not a total, on any row with a nonzero
        // unpriced/partial count — the table shows the counts per row, but
        // a reader skimming the USD column alone would not see that unless
        // told once, plainly, here.
        if result
            .rows
            .iter()
            .any(|row| row.unpriced_calls > 0 || row.partial_calls > 0)
        {
            println!(
                "Rows with nonzero Unpriced/Partial Calls have a USD figure that is a lower \
                 bound, not a total."
            );
        }
    }

    Ok(())
}

/// Render a stored epoch-**millisecond** timestamp as UTC. `spend.rs`'s
/// wire type spells this out in milliseconds (unlike `audit`'s seconds), so
/// this conversion is spelled out here rather than inferred at the call site.
fn format_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map_or_else(|| ms.to_string(), |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::spend::SpendRow;

    fn sample() -> SpendQueryResult {
        SpendQueryResult {
            configured: true,
            period_start_ms: 0,
            period_end_ms: 1,
            rows: vec![SpendRow {
                principal: "u-alice".to_string(),
                usd: 1.23,
                unpriced_calls: 0,
                partial_calls: 2,
            }],
        }
    }

    /// Every column this command renders must be backed by a field the
    /// server actually sends. This is the direct guard against the
    /// `providers list` defect described on `COLUMNS`: it serialises the
    /// *contract type*, not a literal copied from the plan or from memory,
    /// so a rename on either side of the wire shows up here rather than as
    /// a silent dash in the rendered table.
    ///
    /// Falsification: change any `COLUMNS` field-name half to a key that
    /// does not exist (e.g. `"usd"` -> `"usd_amount"`) and this test goes
    /// RED — see task-9-report.md for the pasted failure.
    #[test]
    fn every_rendered_column_is_backed_by_a_real_wire_field() {
        let wire = serde_json::to_value(sample()).unwrap();
        let row_wire = wire["rows"][0]
            .as_object()
            .expect("SpendRow serialises to a JSON object");
        for (header, field) in COLUMNS {
            assert!(
                row_wire.contains_key(*field),
                "column {header:?} claims to render field {field:?}, but SpendRow does not \
                 serialise a key by that name — check {field:?} against \
                 aleph_protocol::spend::SpendRow"
            );
        }
    }

    #[test]
    fn params_serialise_to_an_empty_object() {
        let wire = serde_json::to_value(SpendQueryParams::default()).unwrap();
        assert_eq!(wire, serde_json::json!({}));
    }

    #[test]
    fn period_is_rendered_as_utc_not_a_raw_epoch_number() {
        // 2024-01-01T00:00:00Z
        assert_eq!(format_ms(1_704_067_200_000), "2024-01-01 00:00:00 UTC");
    }
}
